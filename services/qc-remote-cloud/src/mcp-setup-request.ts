/** Browser navigation gets human setup help. MCP transports continue to the
 * OAuth provider so its standards-required 401 challenge remains unchanged. */
export function isMcpSetupPageRequest(request: Request, url = new URL(request.url)): boolean {
  return request.method === "GET"
    && url.pathname === "/mcp"
    && !request.headers.has("authorization")
    && (request.headers.get("accept") ?? "").toLowerCase().includes("text/html");
}
