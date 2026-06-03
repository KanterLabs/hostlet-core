/**
 * Sanitizes the raw `error` query param before it is shown on the login page.
 *
 * The auth backend redirects to `/login?error=<message>` with short, known
 * messages (e.g. "missing oauth code"). Those pass through untouched. Anything
 * unexpected — empty, whitespace-only, or pathologically long values someone
 * tacked onto the URL — is normalized so it can't distort the layout.
 */

// Long enough to preserve every known backend message verbatim, short enough
// that an arbitrary URL value can't overflow the notice.
const MAX_LENGTH = 160;

export function loginErrorFromParams(search: string): string {
  const raw = new URLSearchParams(search).get("error") ?? "";
  const trimmed = raw.replace(/\s+/g, " ").trim();
  if (!trimmed) return "";
  return trimmed.length > MAX_LENGTH ? `${trimmed.slice(0, MAX_LENGTH - 1)}…` : trimmed;
}
