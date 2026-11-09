import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { ArrowRight } from "lucide-react";
import { USING_MOCKS } from "@/lib/api";
import { MOCK_PASSWORD } from "@/lib/mock";
import { errorMessage } from "@/lib/http";
import { useAuth } from "@/state/auth";
import { AuthLayout } from "@/components/auth-layout";
import { Button, Field, Input } from "@/components/ui/controls";

export function LoginScreen() {
  const { signIn, setup } = useAuth();
  const navigate = useNavigate();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await signIn(username, password);
      navigate("/", { replace: true });
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <AuthLayout
      statement="The shelf you actually own."
      note="Ludex indexes the archives on your server and serves them to your machines. No storefront, no account anywhere else."
    >
      <form onSubmit={submit} className="flex flex-col gap-6">
        <div className="flex flex-col gap-2">
          <h1 className="text-[34px] font-semibold leading-none tracking-[-0.03em]">Sign in</h1>
          <p className="flex items-center gap-2 text-[13px] text-ink-3">
            <span>{setup?.serverName ?? "Ludex"}</span>
            <span className="h-3 w-px bg-line-2" aria-hidden="true" />
            <span className="num">v{setup?.version ?? "0.1.0"}</span>
          </p>
        </div>

        <div className="flex flex-col gap-4">
          <Field label="Username" htmlFor="login-username">
            <Input
              id="login-username"
              name="username"
              autoComplete="username"
              autoFocus
              required
              value={username}
              onChange={(event) => setUsername(event.target.value)}
              placeholder="Your username"
            />
          </Field>
          <Field label="Password" htmlFor="login-password">
            <Input
              id="login-password"
              name="password"
              type="password"
              autoComplete="current-password"
              required
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              placeholder="Your password"
            />
          </Field>
        </div>

        {error && (
          <p role="alert" className="border-l-2 border-danger pl-3 text-[13px] text-danger">
            {error}
          </p>
        )}

        <Button type="submit" variant="primary" size="lg" loading={busy} iconRight={<ArrowRight className="size-4" />}>
          Sign in
        </Button>

        {USING_MOCKS && (
          <div className="border-t border-line pt-4">
            <p className="text-[12.5px] leading-relaxed text-ink-3">
              Demo data is on. Sign in as <span className="font-mono text-ink-2">mara</span> with the password{" "}
              <span className="font-mono text-ink-2">{MOCK_PASSWORD}</span>.
            </p>
            <Button
              variant="quiet"
              size="sm"
              className="mt-2 px-0 text-ink-2"
              onClick={() => {
                setUsername("mara");
                setPassword(MOCK_PASSWORD);
                setError(null);
              }}
            >
              Fill demo credentials
            </Button>
          </div>
        )}
      </form>
    </AuthLayout>
  );
}
