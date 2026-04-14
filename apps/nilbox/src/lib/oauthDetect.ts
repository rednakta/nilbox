// ANSI escape sequence removal (SGR + OSC 8 hyperlinks)
const ANSI_RE = /\x1b(?:\[[0-9;]*[a-zA-Z]|\]8;[^\x1b]*\x1b\\)/g;

// OAuth authorize URL pattern — matches /authorize or /auth as complete path segments + response_type=code
const OAUTH_URL_RE =
  /https?:\/\/[^\s"'<>\x00-\x1f]+\/(?:oauth\/)?(?:authorize|auth)[?/][^\s"'<>\x00-\x1f]*response_type=code[^\s"'<>\x00-\x1f]*/g;

/// Max number of captured URLs kept for dedup (LRU cap, insertion order).
const RECENT_URLS_MAX = 10;

/**
 * Detect OAuth authorization URL from a terminal output chunk.
 * Maintains a rolling buffer to handle URLs split across chunks.
 * A captured URL is recorded exactly once — subsequent matches of the
 * same URL return null without updating the cache entry.
 */
export function detectOAuthUrl(
  chunk: Uint8Array,
  buffer: string,
  recentUrls: Map<string, number>,
): { newBuffer: string; detectedUrl: string | null } {
  const text = new TextDecoder().decode(chunk);
  const combined = buffer + text;
  const stripped = combined.replace(ANSI_RE, "");

  // Reset regex state (global flag)
  OAUTH_URL_RE.lastIndex = 0;
  const match = OAUTH_URL_RE.exec(stripped);
  let detectedUrl: string | null = null;
  if (match) {
    const url = match[0];
    if (!recentUrls.has(url)) {
      recentUrls.set(url, Date.now());
      // Evict oldest entries once we exceed the cap (Map preserves insertion order).
      while (recentUrls.size > RECENT_URLS_MAX) {
        const oldestKey = recentUrls.keys().next().value;
        if (oldestKey === undefined) break;
        recentUrls.delete(oldestKey);
      }
      detectedUrl = url;
    }
    // Duplicate capture: do nothing — the URL is already recorded and must
    // not be stored again or have its position refreshed.
  }

  // Trim buffer to last 2KB
  const newBuffer = combined.length > 2048 ? combined.slice(-2048) : combined;
  return { newBuffer, detectedUrl };
}

/**
 * Scan terminal scrollback text for all OAuth authorization URLs.
 * Returns deduplicated list of URLs found.
 */
export function scanBufferForOAuthUrls(text: string): string[] {
  const stripped = text.replace(ANSI_RE, "");
  const matches: string[] = [];
  const re = new RegExp(OAUTH_URL_RE.source, "g");
  let m: RegExpExecArray | null;
  while ((m = re.exec(stripped)) !== null) {
    if (!matches.includes(m[0])) matches.push(m[0]);
  }
  return matches;
}
