/** What the consent screen can truthfully say about the app asking for access.
 *
 * A client's name is whatever it registered - dynamic client registration and
 * client-ID metadata documents both let any party call itself "Claude". The
 * one thing the consent decision actually sends somewhere is the authorization
 * code, and it goes to the redirect URI. So the page names that host, and the
 * origin that published a client-ID metadata document, beside the name, and it
 * recognises only hosts it can vouch for: the redirect hosts the Claude and
 * ChatGPT connector platforms use, and loopback addresses, where the code
 * never leaves the person's own machine. Everything else is shown with a
 * warning rather than refused, because any OAuth-capable MCP app may connect. */

export type RedirectRecognition =
  | { kind: "first-party"; host: string; vendor: string }
  | { kind: "loopback"; host: string }
  | { kind: "unrecognised"; host: string };

/** Redirect hosts of the AI apps QC Remote documents as connector clients.
 *
 * Claude's connectors return to `https://claude.ai/api/mcp/auth_callback`
 * (and the same path on claude.com); ChatGPT's to
 * `https://chatgpt.com/connector_platform_oauth_redirect`, with OpenAI's
 * platform on openai.com. A subdomain of one of these is the same operator. */
const FIRST_PARTY_REDIRECT_DOMAINS: readonly { domain: string; vendor: string }[] = [
  { domain: "claude.ai", vendor: "Claude by Anthropic" },
  { domain: "claude.com", vendor: "Claude by Anthropic" },
  { domain: "chatgpt.com", vendor: "ChatGPT by OpenAI" },
  { domain: "openai.com", vendor: "ChatGPT by OpenAI" }
];

const LOOPBACK_HOSTNAMES = new Set(["localhost", "127.0.0.1", "[::1]"]);

export function recogniseRedirect(redirectUri: string): RedirectRecognition {
  let url: URL;
  try { url = new URL(redirectUri); } catch { return { kind: "unrecognised", host: redirectUri.slice(0, 120) || "an unreadable address" }; }
  const hostname = url.hostname.toLowerCase();
  if ((url.protocol === "http:" || url.protocol === "https:") && LOOPBACK_HOSTNAMES.has(hostname)) {
    return { kind: "loopback", host: url.host };
  }
  if (url.protocol === "https:" && !url.username && !url.password) {
    const match = FIRST_PARTY_REDIRECT_DOMAINS.find(({ domain }) => hostname === domain || hostname.endsWith(`.${domain}`));
    if (match) return { kind: "first-party", host: url.host, vendor: match.vendor };
  }
  // A custom scheme (`someapp://cb`) has no host; name the scheme instead.
  return { kind: "unrecognised", host: url.host || `${url.protocol}//` };
}

/** The origin that published a client-ID metadata document, when the client
 * id is one (an https URL); undefined for a dynamically registered client. */
export function clientIdMetadataOrigin(clientId: string): string | undefined {
  try {
    const url = new URL(clientId);
    return url.protocol === "https:" ? url.origin : undefined;
  } catch {
    return undefined;
  }
}
