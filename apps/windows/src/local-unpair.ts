export async function completeUnpairIfSuccessful(
  remove: () => boolean | Promise<boolean>,
  onComplete: () => void
): Promise<boolean> {
  try {
    if (!(await remove())) return false;
    onComplete();
    return true;
  } catch {
    return false;
  }
}
