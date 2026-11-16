#!/usr/bin/env python3
"""WebKitGTK Gamepad API + frame-budget spike for Console 2.0 (#270 §4).

Runs the built Console in a real WebKitGTK view (the engine the Tauri Linux
build ships) under Xvfb, so the Gamepad API surface and the rail frame budget
are measured on the engine itself, not in Chromium.

    scripts/e2e.sh &                     # or any harness with a seeded library
    source <harness env>                 # LUDEX_E2E_BASE_URL / LUDEX_E2E_TOKEN
    xvfb-run -a python3 e2e/webkitgtk-spike.py report.json

What this host can prove:
  * whether `navigator.getGamepads` exists in WebKitGTK;
  * the rail frame budget on WebKitGTK's software rasterizer under Xvfb.

What it cannot prove without a device: `mapping === "standard"`, button
decoding, hot-plug and timestamp advancement. Those need a real controller
(`/dev/input/js*`) or a `/dev/uinput` virtual pad; the report says so instead
of guessing. See docs/console/WEBKITGTK.md for the manual procedure.
"""

from __future__ import annotations

import json
import os
import sys
import time

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.1")
from gi.repository import GLib, Gtk, WebKit2  # noqa: E402

SCROLL_MS = 3_000
PRESS_GAP_MS = 90

# Injected at document start: the token must exist before the app boots, and
# the sampler must wrap requestAnimationFrame before the input loop starts.
INIT_SCRIPT = """
(() => {
  try { localStorage.setItem("ludex.token", __TOKEN__); } catch (error) {}
  const state = {
    deltas: [],
    recording: false,
    last: 0,
    longTasks: [],
    chains: new Map(),
    calls: [],
    nextChain: 1,
    currentChain: null,
  };
  const original = window.requestAnimationFrame.bind(window);
  const sampler = (now) => {
    if (state.recording) {
      if (state.last > 0) state.deltas.push(now - state.last);
      state.last = now;
    }
    original(sampler);
  };
  original(sampler);
  window.requestAnimationFrame = (callback) => {
    const chain = state.currentChain === null ? state.nextChain++ : state.currentChain;
    const now = performance.now();
    state.chains.set(chain, now);
    state.calls.push(now);
    if (state.calls.length > 8000) state.calls.splice(0, state.calls.length - 8000);
    return original((time) => {
      const previous = state.currentChain;
      state.currentChain = chain;
      try { callback(time); } finally { state.currentChain = previous; }
    });
  };
  try {
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        state.longTasks.push({ start: entry.startTime, duration: entry.duration });
      }
    }).observe({ type: "longtask", buffered: true });
  } catch (error) {}
  window.__spike = {
    start() { state.deltas = []; state.last = 0; state.recording = true; },
    stop() { state.recording = false; return state.deltas.slice(); },
    longTasks() { return state.longTasks.slice(); },
    raf() {
      const since = performance.now() - 1000;
      let chains = 0;
      for (const at of state.chains.values()) if (at >= since) chains += 1;
      return {
        chains,
        callbacks: state.calls.filter((at) => at >= since).length,
      };
    },
    dispatch() {
      window.dispatchEvent(new KeyboardEvent("keydown", {
        key: "ArrowRight", code: "ArrowRight", bubbles: true,
      }));
    },
    scroll(ms, gap) {
      window.__spike.start();
      let sent = 0;
      const timer = setInterval(() => {
        window.__spike.dispatch();
        sent += 1;
        if (sent * gap >= ms) {
          clearInterval(timer);
          window.__spike.result = { deltas: window.__spike.stop(), sent };
        }
      }, gap);
    },
  };
})();
"""


class Spike:
    def __init__(self, base: str, token: str, width: int = 1920, height: int = 1080):
        self.base = base.rstrip("/")
        self.token = token
        self.width = width
        self.height = height
        self.result: dict = {}

        self.window = Gtk.Window()
        self.window.set_default_size(width, height)
        manager = WebKit2.UserContentManager()
        manager.add_script(
            WebKit2.UserScript.new(
                INIT_SCRIPT.replace("__TOKEN__", json.dumps(token)),
                WebKit2.UserScriptInjectionTime.START,
                WebKit2.UserContentInjectedFrames.ALL_FRAMES,
                None,
                None,
            )
        )
        self.view = WebKit2.WebView.new_with_user_content_manager(manager)
        self.window.add(self.view)
        self.window.show_all()
        self.loaded = False
        self.view.connect("load-changed", self._on_load)

    # -- GLib plumbing -----------------------------------------------------
    def _on_load(self, _view, event):
        if event == WebKit2.LoadEvent.FINISHED:
            self.loaded = True

    def pump(self, seconds: float) -> None:
        context = GLib.MainContext.default()
        end = time.time() + seconds
        while time.time() < end:
            while context.pending():
                context.iteration(False)
            time.sleep(0.01)

    def eval_js(self, script: str, wait: float = 20.0):
        """Synchronous `evaluate_javascript`: a private loop + a timeout."""
        holder: dict = {}
        loop = GLib.MainLoop()

        def done(view, result, _data):
            try:
                value = view.evaluate_javascript_finish(result)
                raw = value.to_json(0)
                holder["value"] = json.loads(raw) if raw is not None else None
            except Exception as exc:  # noqa: BLE001
                holder["error"] = repr(exc)
            if loop.is_running():
                loop.quit()

        self.view.evaluate_javascript(script, -1, None, None, None, done, None)
        timeout = GLib.timeout_add_seconds(int(wait) or 1, lambda: (loop.quit(), False)[1])
        loop.run()
        GLib.source_remove(timeout)
        if "error" in holder:
            raise RuntimeError(holder["error"])
        return holder.get("value")

    def wait_for(self, script: str, timeout: float = 30.0, label: str = "condition") -> None:
        end = time.time() + timeout
        while time.time() < end:
            if self.eval_js(script, wait=timeout) is True:
                return
            self.pump(0.25)
        raise TimeoutError(label)

    # -- checks ------------------------------------------------------------
    def run(self) -> dict:
        self.view.load_uri(f"{self.base}/#/console")
        self.wait_for("window.__spike !== undefined", label="spike init script")

        api = self.eval_js(
            """(() => ({
              hasGamepads: typeof navigator.getGamepads,
              padCount: (() => {
                try { const list = navigator.getGamepads(); return list ? list.length : null; }
                catch (error) { return null; }
              })(),
              standardMappingSeen: (() => {
                try { return Array.from(navigator.getGamepads() || []).some((pad) => pad && pad.mapping === "standard"); }
                catch (error) { return null; }
              })(),
              userAgent: navigator.userAgent,
            }))()"""
        )

        # The Console gates on auth + library: wait for a real cover.
        self.wait_for(
            "document.querySelectorAll('[data-bp-card=\"true\"]').length > 0",
            label="console cover",
        )
        self.pump(1.5)
        route = self.eval_js("document.querySelector('[data-console-route]')?.dataset.consoleRoute ?? null")
        layout = self.eval_js("document.querySelector('[data-bp-layout]')?.dataset.bpLayout ?? null")
        room = self.eval_js("document.querySelector('.bp-root')?.dataset.consoleRoom ?? null")
        selected_before = self.eval_js(
            "document.querySelector('[data-bp-card=\"true\"][aria-current=\"true\"]')?.getAttribute('aria-label') ?? null"
        )

        # Idle window: one second of frames with no input.
        self.eval_js("window.__spike.start()")
        self.pump(1.0)
        idle = self.eval_js("window.__spike.stop()")

        self.eval_js(f"window.__spike.scroll({SCROLL_MS}, {PRESS_GAP_MS})")
        scroll_result = None
        end = time.time() + (SCROLL_MS / 1000) + 10
        while time.time() < end:
            scroll_result = self.eval_js("window.__spike.result ?? null", wait=10)
            if scroll_result:
                break
            self.pump(0.25)
        if not scroll_result:
            raise TimeoutError("scroll measurement")

        selected_after = self.eval_js(
            "document.querySelector('[data-bp-card=\"true\"][aria-current=\"true\"]')?.getAttribute('aria-label') ?? null"
        )
        self.pump(3.0)
        raf = self.eval_js("window.__spike.raf()")
        long_tasks = self.eval_js("window.__spike.longTasks()")

        deltas = scroll_result["deltas"]

        def summary(values):
            ordered = sorted(values)
            if not ordered:
                return {"frames": 0, "p50": 0, "p95": 0, "max": 0, "over33": 0}
            def pct(fraction):
                index = min(len(ordered) - 1, max(0, int(len(ordered) * fraction + 0.999) - 1))
                return round(ordered[index], 1)
            return {
                "frames": len(ordered),
                "p50": pct(0.5),
                "p95": pct(0.95),
                "max": round(ordered[-1], 1),
                "over33": sum(1 for value in values if value > 1000 / 30),
            }

        return {
            "engine": "WebKitGTK",
            "engineVersion": f"{WebKit2.get_major_version()}.{WebKit2.get_minor_version()}.{WebKit2.get_micro_version()}",
            "viewport": {"width": self.width, "height": self.height},
            "display": os.environ.get("DISPLAY"),
            "api": api,
            "console": {"route": route, "layout": layout, "room": room},
            "idle": summary(idle),
            "scroll": {
                **summary(deltas),
                "sent": scroll_result["sent"],
                "from": selected_before,
                "to": selected_after,
                # A rail that did not move means the library is too small (or
                # input did not reach the shell): every frame number is idle.
                "moved": selected_before is not None and selected_before != selected_after,
            },
            "rAF": raf,
            "longTasks": {
                "count": len(long_tasks),
                "totalMs": round(sum(entry["duration"] for entry in long_tasks), 1),
                "longestMs": round(max((entry["duration"] for entry in long_tasks), default=0), 1),
            },
            "blockedChecks": [
                "mapping === 'standard' needs a controller",
                "button/axis decoding needs a controller",
                "hot-plug needs a controller",
                "timestamp advancement needs a controller",
            ],
        }


def main() -> int:
    base = os.environ.get("LUDEX_E2E_BASE_URL")
    token = os.environ.get("LUDEX_E2E_TOKEN")
    if not base or not token:
        print("LUDEX_E2E_BASE_URL and LUDEX_E2E_TOKEN are required", file=sys.stderr)
        return 2

    spike = Spike(base, token)
    report = spike.run()
    if not report["scroll"]["moved"]:
        print(
            "warning: the rail did not move — the library is too small or input "
            "did not reach the shell; the scroll numbers are idle frames",
            file=sys.stderr,
        )
    text = json.dumps(report, indent=2, ensure_ascii=False)
    print(text)
    if len(sys.argv) > 1:
        with open(sys.argv[1], "w", encoding="utf-8") as handle:
            handle.write(text + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
