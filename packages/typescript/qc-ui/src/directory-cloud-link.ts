/**
 * Whether the unit's Neural DSP cloud account is linked, read from the Directory dialog's own top-bar indicator in its
 * graphics tree. The unit draws the cloud-upload button (`image: 'icons/24/cloud_upload.png'` inside a
 * `zenUI::PushButtonIndicator`) when the account is linked; the host is told nothing else about the account, so the
 * unit's picture is the only live source (observed on CorOS 4.1.0 with a linked account: the unit showed the upload
 * button and an enabled Cloud Presets row while the app drew the signed-out glyph and a dimmed row).
 *
 * Only the class and image names are read, never a `text :` line, so no account detail can reach the result.
 *
 * The result is tri-state. `true` is read from the cloud-upload image. `false` is read only from an image registered in
 * {@link QC_DIRECTORY_UNLINKED_IMAGES}, which is empty until the signed-out indicator's image name has been read from a
 * tree: the name is not guessed. Anything else (not the Directory, a frame mid-transition) is `undefined`, and the app
 * keeps what it last read, falling back to its signed-out drawing when it has read nothing.
 */
export const QC_DIRECTORY_LINKED_IMAGES: ReadonlySet<string> = new Set(["icons/24/cloud_upload.png"]);
export const QC_DIRECTORY_UNLINKED_IMAGES: ReadonlySet<string> = new Set();

const CLASS_LINE = /^(\s*)zenUI::([A-Za-z0-9_]+)\s*$/;
const IMAGE_LINE = /^(\s*)image\s*:\s*'([^']*)'\s*$/;

export function directoryCloudSignedInFromTree(tree: string): boolean | undefined {
  if (!/^\s*zenUI::DirectoryDialog\s*$/m.test(tree)) return undefined;
  const stack: { indent: number; name: string }[] = [];
  let linked = false;
  let unlinked = false;
  for (const line of tree.split(/\r?\n/)) {
    const node = line.match(CLASS_LINE);
    if (node) {
      const indent = node[1].length;
      while (stack.length && stack[stack.length - 1].indent >= indent) stack.pop();
      stack.push({ indent, name: node[2] });
      continue;
    }
    const image = line.match(IMAGE_LINE);
    if (!image) continue;
    const indent = image[1].length;
    const ancestors = stack.filter((entry) => entry.indent < indent).map((entry) => entry.name);
    if (!ancestors.includes("DirectoryDialog") || !ancestors.includes("PushButtonIndicator")) continue;
    if (QC_DIRECTORY_LINKED_IMAGES.has(image[2])) linked = true;
    else if (QC_DIRECTORY_UNLINKED_IMAGES.has(image[2])) unlinked = true;
  }
  // Both at once is a frame the indicator is mid-change in: say nothing rather than guess.
  return linked === unlinked ? undefined : linked;
}
