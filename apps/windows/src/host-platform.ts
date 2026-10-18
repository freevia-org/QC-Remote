export function hostPlatform(navigatorInfo: { platform?: string; userAgent?: string; userAgentData?: { platform?: string } }): string | undefined {
  const platform = navigatorInfo.userAgentData?.platform || navigatorInfo.platform || navigatorInfo.userAgent || "";
  if (/mac/i.test(platform)) return "macOS";
  if (/win/i.test(platform)) return "Windows";
  if (/linux/i.test(platform)) return "Linux";
  return undefined;
}

export const desktopPlatform = typeof navigator === "undefined" ? undefined : hostPlatform(navigator);
export function applicationModifier(event: { ctrlKey: boolean; metaKey: boolean }, platform = desktopPlatform): boolean {
  return event.ctrlKey || (platform === "macOS" && event.metaKey);
}
export function shortcutLabel(shortcut: string): string {
  return desktopPlatform === "macOS" ? shortcut.replace("Ctrl+", "⌘").replace("Shift+", "⇧") : shortcut;
}
