/** Provider event times and login times are seconds. This non-identifying fence
 * survives account deletion so old notifications cannot delete a new account. */
export const APPLE_ACCOUNT_EPOCH_KEY = "apple-account-epoch-v1";
export type AppleAccountEpoch = { signedInAt: number; revokedAt: number };
export const shouldApplyAppleEvent = (epoch: AppleAccountEpoch | undefined, eventTime: number): boolean =>
  eventTime >= (epoch?.signedInAt ?? 0) && eventTime > (epoch?.revokedAt ?? 0);
