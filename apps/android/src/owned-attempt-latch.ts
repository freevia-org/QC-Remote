/** Owns one in-flight asynchronous attempt. A stale completion may release
 * only the latch it acquired, never a newer attempt's latch. */
export class OwnedAttemptLatch {
  private nextOwner = 1;
  private owner: number | undefined;

  get isHeld(): boolean { return this.owner !== undefined; }

  acquire(): number | undefined {
    if (this.owner !== undefined) return undefined;
    const owner = this.nextOwner++;
    this.owner = owner;
    return owner;
  }

  release(owner: number): boolean {
    if (this.owner !== owner) return false;
    this.owner = undefined;
    return true;
  }

  clear(): void {
    this.owner = undefined;
    this.nextOwner += 1;
  }
}
