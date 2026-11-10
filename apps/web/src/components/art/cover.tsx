import type { CSSProperties } from "react";
import { ART_BY_SLUG, MOTIF_KEYS, PALETTES, artFromUrl, artForTitle, hashString, mulberry32, type ArtSpec, type MotifKey } from "@/lib/art";
import { assetUrl } from "@/lib/api";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";

/* --------------------------------------------------------------------------
   Motifs. Every plate is drawn in a 300x400 space and cropped by CSS, so the
   same drawing works as a 44px thumbnail and as a full-bleed backdrop.
   The bottom band always resolves to the base colour: the title lives there.
   -------------------------------------------------------------------------- */

function Motif({ spec, seed }: { spec: ArtSpec; seed: number }) {
  const p = PALETTES[spec.palette];
  const rand = mulberry32(seed);
  const r = (min: number, max: number) => min + rand() * (max - min);

  const footer = <rect x="0" y="258" width="300" height="142" fill={p.base} opacity="0.55" />;

  switch (spec.motif) {
    case "grid": {
      const cells: React.ReactNode[] = [];
      for (let x = 0; x < 5; x++) {
        for (let y = 0; y < 7; y++) {
          if (rand() > 0.72) {
            cells.push(
              <rect
                key={`${x}-${y}`}
                x={x * 60 + 2}
                y={y * 57 + 2}
                width={56}
                height={53}
                fill={rand() > 0.78 ? p.accent : p.base}
                opacity={r(0.25, 0.8)}
              />,
            );
          }
        }
      }
      return (
        <g>
          <rect width="300" height="400" fill={p.field} />
          <g stroke={p.paper} strokeWidth="1" opacity="0.32">
            {Array.from({ length: 6 }, (_, i) => (
              <line key={`v${i}`} x1={i * 60} y1="0" x2={i * 60} y2="400" />
            ))}
            {Array.from({ length: 8 }, (_, i) => (
              <line key={`h${i}`} x1="0" y1={i * 57} x2="300" y2={i * 57} />
            ))}
          </g>
          {cells}
          <rect x="0" y="342" width="300" height="58" fill={p.base} opacity="0.9" />
          {footer}
        </g>
      );
    }

    case "halftone": {
      const dots: React.ReactNode[] = [];
      for (let x = 0; x < 11; x++) {
        for (let y = 0; y < 15; y++) {
          const d = Math.hypot(x - 3, y - 4);
          const radius = Math.max(0.6, 7.2 - d * 0.62);
          if (radius > 0.9) {
            dots.push(<circle key={`${x}-${y}`} cx={x * 28 + 14} cy={y * 27 + 12} r={radius} fill={p.paper} opacity="0.55" />);
          }
        }
      }
      return (
        <g>
          <rect width="300" height="400" fill={p.base} />
          {dots}
          <path d="M0 250 L300 120 L300 400 L0 400 Z" fill={p.field} opacity="0.92" />
          <circle cx="228" cy="300" r="46" fill={p.accent} opacity="0.85" />
          {footer}
        </g>
      );
    }

    case "chevron": {
      const bands: React.ReactNode[] = [];
      for (let i = 0; i < 9; i++) {
        const y = 30 + i * 38;
        bands.push(
          <path
            key={i}
            d={`M-20 ${y} L150 ${y - 74} L320 ${y}`}
            stroke={i === 4 ? p.accent : p.paper}
            strokeWidth={i === 4 ? 26 : 20}
            fill="none"
            opacity={i === 4 ? 0.9 : 0.16 + i * 0.03}
          />,
        );
      }
      return (
        <g>
          <rect width="300" height="400" fill={p.base} />
          <rect width="300" height="400" fill={p.field} opacity="0.55" />
          {bands}
          {footer}
        </g>
      );
    }

    case "arcs": {
      const rings: React.ReactNode[] = [];
      for (let i = 0; i < 15; i++) {
        const rad = 40 + i * 26;
        rings.push(
          <circle
            key={i}
            cx="26"
            cy="376"
            r={rad}
            fill="none"
            stroke={i % 5 === 0 ? p.accent : p.paper}
            strokeWidth={i % 5 === 0 ? 3 : 1.1}
            opacity={i % 5 === 0 ? 0.9 : 0.3}
          />,
        );
      }
      return (
        <g>
          <rect width="300" height="400" fill={p.field} />
          {rings}
          <circle cx="252" cy="76" r="16" fill={p.accent} />
          {footer}
        </g>
      );
    }

    case "horizon": {
      return (
        <g>
          <rect width="300" height="400" fill={p.base} />
          <rect width="300" height="252" fill={p.field} />
          <circle cx="198" cy="200" r="54" fill={p.paper} opacity="0.9" />
          <rect x="0" y="200" width="300" height="50" fill={p.field} />
          <rect x="0" y="250" width="300" height="150" fill={p.base} />
          <g stroke={p.paper} opacity="0.28">
            {[268, 286, 306, 328, 352].map((y) => (
              <line key={y} x1="0" y1={y} x2="300" y2={y} />
            ))}
          </g>
          <rect x="34" y="268" width="84" height="3" fill={p.accent} opacity="0.9" />
          {footer}
        </g>
      );
    }

    case "cross": {
      return (
        <g>
          <rect width="300" height="400" fill={p.base} />
          <rect x="0" y="0" width="300" height="400" fill={p.field} opacity="0.45" />
          <g stroke={p.paper} opacity="0.16">
            {Array.from({ length: 10 }, (_, i) => (
              <line key={`v${i}`} x1={i * 33} y1="0" x2={i * 33} y2="400" />
            ))}
            {Array.from({ length: 14 }, (_, i) => (
              <line key={`h${i}`} x1="0" y1={i * 31} x2="300" y2={i * 31} />
            ))}
          </g>
          <circle cx="150" cy="170" r="74" fill="none" stroke={p.paper} strokeWidth="1.4" opacity="0.75" />
          <circle cx="150" cy="170" r="26" fill="none" stroke={p.accent} strokeWidth="1.4" opacity="0.9" />
          <line x1="150" y1="52" x2="150" y2="288" stroke={p.paper} strokeWidth="1" opacity="0.55" />
          <line x1="32" y1="170" x2="268" y2="170" stroke={p.paper} strokeWidth="1" opacity="0.55" />
          <rect x="146" y="166" width="8" height="8" fill={p.accent} />
          {footer}
        </g>
      );
    }

    case "split": {
      return (
        <g>
          <rect width="300" height="400" fill={p.base} />
          <path d="M0 0 L300 0 L300 400 L0 400 Z" fill={p.field} opacity="0.9" />
          <path d="M300 0 L0 0 L0 400 Z" fill={p.base} opacity="0.35" />
          <path d="M0 0 L300 400" stroke={p.accent} strokeWidth="2" opacity="0.8" />
          <circle cx="212" cy="120" r="64" fill="none" stroke={p.paper} strokeWidth="1.2" opacity="0.7" />
          <circle cx="212" cy="120" r="26" fill={p.accent} opacity="0.9" />
          {footer}
        </g>
      );
    }

    case "type": {
      return (
        <g>
          <rect width="300" height="400" fill={p.base} />
          <text
            x="16"
            y="306"
            style={{ fontFamily: "var(--font-display)", fontWeight: 700 }}
            fontSize="312"
            fill={p.paper}
            opacity="0.9"
          >
            {spec.letter ?? "L"}
          </text>
          <line x1="22" y1="44" x2="278" y2="44" stroke={p.paper} strokeWidth="1" opacity="0.5" />
          <rect x="22" y="60" width="30" height="30" fill={p.accent} />
          <rect x="0" y="258" width="300" height="142" fill={p.base} opacity="0.5" />
        </g>
      );
    }

    case "topo": {
      const rings: React.ReactNode[] = [];
      for (let i = 0; i < 12; i++) {
        const R = 26 + i * 21;
        const phase = rand() * Math.PI * 2;
        const pts: string[] = [];
        for (let a = 0; a <= 360; a += 6) {
          const rad = (a * Math.PI) / 180;
          const wobble = 1 + 0.1 * Math.sin(3 * rad + phase) + 0.055 * Math.sin(7 * rad + phase * 2);
          const x = 150 + Math.cos(rad) * R * wobble;
          const y = 176 + Math.sin(rad) * R * wobble * 0.92;
          pts.push(`${x.toFixed(1)},${y.toFixed(1)}`);
        }
        rings.push(
          <polygon
            key={i}
            points={pts.join(" ")}
            fill="none"
            stroke={i === 6 ? p.accent : p.paper}
            strokeWidth={i === 6 ? 2.2 : 0.9}
            opacity={i === 6 ? 0.95 : 0.3}
          />,
        );
      }
      return (
        <g>
          <rect width="300" height="400" fill={p.base} />
          <rect width="300" height="400" fill={p.field} opacity="0.4" />
          {rings}
          {footer}
        </g>
      );
    }

    case "waves": {
      const lines: React.ReactNode[] = [];
      for (let i = 0; i < 14; i++) {
        const y = 40 + i * 18;
        const amp = 8 + (i % 4) * 5;
        const pts: string[] = [];
        for (let x = 0; x <= 300; x += 6) {
          pts.push(`${x},${(y + Math.sin((x / 46) + i * 0.7) * amp).toFixed(1)}`);
        }
        lines.push(
          <polyline
            key={i}
            points={pts.join(" ")}
            fill="none"
            stroke={i === 8 ? p.accent : p.paper}
            strokeWidth={i === 8 ? 2 : 0.9}
            opacity={i === 8 ? 0.9 : 0.26}
          />,
        );
      }
      return (
        <g>
          <rect width="300" height="400" fill={p.base} />
          <rect y="120" width="300" height="280" fill={p.field} opacity="0.55" />
          {lines}
          {footer}
        </g>
      );
    }

    case "blocks": {
      const blocks: React.ReactNode[] = [];
      for (let i = 0; i < 7; i++) {
        const w = r(70, 150);
        const x = r(-30, 190);
        const y = r(20, 260);
        blocks.push(
          <rect
            key={i}
            x={x}
            y={y}
            width={w}
            height={w * 0.62}
            fill={i % 3 === 0 ? p.accent : p.paper}
            opacity={i % 3 === 0 ? 0.75 : r(0.08, 0.2)}
            transform={`skewY(-14)`}
          />,
        );
      }
      return (
        <g>
          <rect width="300" height="400" fill={p.base} />
          <rect width="300" height="400" fill={p.field} opacity="0.65" />
          {blocks}
          {footer}
        </g>
      );
    }

    case "orbit": {
      return (
        <g>
          <rect width="300" height="400" fill={p.base} />
          <circle cx="150" cy="172" r="104" fill={p.field} opacity="0.85" />
          <ellipse cx="150" cy="172" rx="142" ry="52" fill="none" stroke={p.paper} strokeWidth="1.1" opacity="0.5" transform="rotate(-22 150 172)" />
          <ellipse cx="150" cy="172" rx="128" ry="118" fill="none" stroke={p.paper} strokeWidth="1" opacity="0.28" />
          <circle cx="252" cy="112" r="12" fill={p.accent} />
          <circle cx="150" cy="172" r="46" fill={p.base} opacity="0.55" />
          <g stroke={p.paper} opacity="0.4">
            {Array.from({ length: 24 }, (_, i) => {
              const a = (i / 24) * Math.PI * 2;
              return (
                <line
                  key={i}
                  x1={150 + Math.cos(a) * 132}
                  y1={172 + Math.sin(a) * 132}
                  x2={150 + Math.cos(a) * (i % 6 === 0 ? 120 : 126)}
                  y2={172 + Math.sin(a) * (i % 6 === 0 ? 120 : 126)}
                />
              );
            })}
          </g>
          {footer}
        </g>
      );
    }
  }
}

const plateStyle = (spec: ArtSpec): CSSProperties => {
  const p = PALETTES[spec.palette];
  return {
    "--c-base": p.base,
    "--c-field": p.field,
    "--c-accent": p.accent,
    "--c-paper": p.paper,
  } as CSSProperties;
};

/* --------------------------------- cover --------------------------------- */

export function GameCover({
  title,
  coverUrl,
  index,
  year,
  genre,
  className,
  showType = true,
}: {
  title: string;
  coverUrl: string | null;
  index?: number;
  year?: string | null;
  genre?: string | null;
  className?: string;
  showType?: boolean;
}) {
  const spec = artFromUrl(coverUrl, title);
  const demo = !coverUrl || coverUrl.startsWith("demo://");
  const seed = hashString(coverUrl ?? title);
  const src = assetUrl(coverUrl);

  return (
    <div className={cn("cover", className)} style={plateStyle(spec)}>
      {demo ? (
        <svg className="cover-motif" viewBox="0 0 300 400" preserveAspectRatio="xMidYMid slice" aria-hidden="true">
          <Motif spec={spec} seed={seed} />
        </svg>
      ) : (
        <img
          src={src ?? ""}
          alt=""
          className="absolute inset-0 h-full w-full object-cover transition-transform duration-[240ms] ease-out group-hover:scale-[1.03]"
          loading="lazy"
          onError={(e) => {
            e.currentTarget.style.display = "none";
          }}
        />
      )}
      <div className="cover-sheen" aria-hidden="true" />
      {demo && showType && (
        <>
          <div
            className="pointer-events-none absolute inset-0 z-[1]"
            style={{
              background:
                "linear-gradient(180deg, rgba(0,0,0,.42) 0%, rgba(0,0,0,0) 24%, rgba(0,0,0,0) 52%, rgba(0,0,0,.55) 100%)",
            }}
          />
          <div className="cover-type">
            <div className="cover-series">
              <span>Ludex</span>
              <span>{typeof index === "number" ? `No. ${String(index).padStart(3, "0")}` : t("Archive")}</span>
            </div>
            <div>
              <div className="cover-title">{title}</div>
              <div className="cover-foot">
                {year && <span>{year}</span>}
                {genre && <span>{genre}</span>}
              </div>
            </div>
          </div>
        </>
      )}
    </div>
  );
}

/* ---------------------------------- plate --------------------------------- */

/** Bare artwork, no gradient or copy. Used by the Home hero. */
export function ArtPlate({
  title,
  artUrl,
  className,
}: {
  title: string;
  artUrl: string | null;
  className?: string;
}) {
  const spec = artFromUrl(artUrl, title);
  const demo = !artUrl || artUrl.startsWith("demo://");
  const seed = hashString(artUrl ?? title);
  const src = assetUrl(artUrl);

  return (
    <div className={cn("overflow-hidden bg-bg-deep", className)} style={plateStyle(spec)} aria-hidden="true">
      {demo ? (
        <svg className="h-full w-full" viewBox="0 0 300 400" preserveAspectRatio="xMidYMid slice" aria-hidden="true">
          <Motif spec={spec} seed={seed} />
        </svg>
      ) : (
        <img src={src ?? ""} alt="" className="h-full w-full object-cover" />
      )}
    </div>
  );
}

/* -------------------------------- backdrop -------------------------------- */

export function GameBackdrop({
  title,
  backgroundUrl,
  className,
}: {
  title: string;
  backgroundUrl: string | null;
  className?: string;
}) {
  const spec = artFromUrl(backgroundUrl, title);
  const demo = !backgroundUrl || backgroundUrl.startsWith("demo://");
  const seed = hashString(backgroundUrl ?? title);
  const src = assetUrl(backgroundUrl);

  return (
    <div className={cn("overflow-hidden bg-bg-deep", className)} aria-hidden="true">
      {demo ? (
        <svg
          className="absolute inset-0 h-full w-full opacity-70"
          viewBox="0 0 300 400"
          preserveAspectRatio="xMidYMid slice"
          aria-hidden="true"
        >
          <Motif spec={spec} seed={seed} />
        </svg>
      ) : (
        <img src={src ?? ""} alt="" className="absolute inset-0 h-full w-full object-cover" />
      )}
      <div className="absolute inset-0 bg-[var(--bg)]/30" aria-hidden="true" />
      <div
        className="absolute inset-0"
        style={{
          background:
            "linear-gradient(180deg, rgba(5,5,5,0.35) 0%, rgba(5,5,5,0.12) 40%, var(--bg) 98%)",
        }}
      />
      <div
        className="absolute inset-0"
        style={{ background: "linear-gradient(90deg, rgba(5,5,5,0.9) 0%, rgba(5,5,5,0.68) 45%, rgba(5,5,5,0.3) 75%, rgba(5,5,5,0.08) 100%)" }}
      />
    </div>
  );
}

/* --------------------------------- stills --------------------------------- */

export function StillPlate({
  spec,
  seed,
  index,
  className,
}: {
  spec: ArtSpec;
  seed: number;
  index: number;
  className?: string;
}) {
  // "type" is a poster motif; it crops badly in a 16:9 frame, so stills skip it.
  const stillMotifs: MotifKey[] = MOTIF_KEYS.filter((motif) => motif !== "type");
  const base = Math.max(0, stillMotifs.indexOf(spec.motif));
  const variant: ArtSpec = { ...spec, motif: stillMotifs[(base + index) % stillMotifs.length] };
  return (
    <div className={cn("media-plate aspect-video w-full", className)} style={plateStyle(spec)}>
      <svg className="absolute inset-0 h-full w-full" viewBox="0 0 300 400" preserveAspectRatio="xMidYMid slice" aria-hidden="true">
        <Motif spec={variant} seed={seed + index * 977} />
      </svg>
      <div className="absolute inset-0 bg-[linear-gradient(180deg,rgba(0,0,0,0.1),rgba(0,0,0,0.55))]" />
      <div className="absolute inset-x-0 bottom-0 flex items-end justify-between p-3 font-mono text-[10px] uppercase tracking-[0.14em] text-white/75">
        <span>{variant.motif}</span>
        <span>{t("Still")} {String(index + 1).padStart(2, "0")}</span>
      </div>
    </div>
  );
}

export { ART_BY_SLUG, artForTitle };
