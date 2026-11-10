import { Link } from "react-router-dom";
import { Button } from "@/components/ui/controls";

export function NotFoundScreen() {
  return (
    <div className="page page-in flex min-h-[70vh] flex-col justify-center">
      <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-ink-3">Error 404</p>
      <h1 className="mt-3 max-w-[20ch] text-[clamp(2.5rem,5vw,4.5rem)] font-semibold leading-[0.95] tracking-[-0.035em]">
        This shelf is empty.
      </h1>
      <p className="mt-4 max-w-[46ch] text-[14.5px] leading-relaxed text-ink-2">
        The page you asked for is not in the catalogue. It may have been moved, or the link may be older than this server.
      </p>
      <div className="mt-7 flex flex-wrap gap-2">
        <Link to="/library">
          <Button variant="primary">Back to the library</Button>
        </Link>
        <Link to="/all">
          <Button variant="default">Open the index</Button>
        </Link>
      </div>
    </div>
  );
}
