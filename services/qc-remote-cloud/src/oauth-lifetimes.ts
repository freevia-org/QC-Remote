/** Access tokens remain short-lived. Explicit undefined overrides the provider's
 * defaults: refresh grants and registered clients persist until revoked.
 * Omitting either property would silently restore the 30/90-day defaults. */
export const ACCESS_TOKEN_TTL_SECONDS = 60 * 60;
/** Historical limit, used only to bound pre-persistence deletion receipts. */
export const LEGACY_REFRESH_TOKEN_TTL_SECONDS = 30 * 24 * 60 * 60;

export const OAUTH_TOKEN_LIFETIMES = {
  accessTokenTTL: ACCESS_TOKEN_TTL_SECONDS,
  refreshTokenTTL: undefined,
  clientRegistrationTTL: undefined
};
