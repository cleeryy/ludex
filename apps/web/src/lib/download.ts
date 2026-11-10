import { api, assetUrl, USING_MOCKS } from "@/lib/api";

/** Starts a browser download through a short-lived ticket. Throws a readable error. */
export async function downloadGame(gameId: number, versionId?: number): Promise<void> {
  if (USING_MOCKS) {
    throw new Error("Demo build: downloads need the real server. The browser flow is wired.");
  }
  const ticket = await api.downloadTicket(gameId, versionId);
  const url = assetUrl(ticket.url);
  if (!url) throw new Error("The server returned an empty download URL");
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.rel = "noopener";
  anchor.click();
}
