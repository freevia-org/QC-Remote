/** How long the OAuth credentials this worker issues to AI apps stay valid.
 *
 * These were the OAuth provider library's defaults, inherited silently: a
 * library upgrade that changed a default would have changed what every
 * connector holds without a line of this repository moving. They are now
 * stated, with the same values, so a change is a reviewed edit here.
 *
 * - An access token lasts one hour; a stolen one is useful for at most that.
 * - A grant and its refresh token last thirty days from the initial
 *   authorization code exchange. A refresh rotates the tokens but does not
 *   extend that deadline, so however often a connector is used, its owner
 *   signs in again after thirty days; revoking the grant ends it sooner. */
export const ACCESS_TOKEN_TTL_SECONDS = 60 * 60;
export const REFRESH_TOKEN_TTL_SECONDS = 30 * 24 * 60 * 60;

/** Spread into the OAuthProvider options. */
export const OAUTH_TOKEN_LIFETIMES = {
  accessTokenTTL: ACCESS_TOKEN_TTL_SECONDS,
  refreshTokenTTL: REFRESH_TOKEN_TTL_SECONDS
} as const;
