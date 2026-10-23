# First iOS App Store release

The release is free and offers paired-host control on iPhone and iPad. It does
not offer direct mobile USB or USB-MIDI. The native DriverKit prototype remains
in source but is not an App target dependency, is not embedded, and has no app
entitlement in this release. Legacy route preferences cannot select the driver.

## Apple sign-in configuration

The cloud offers Apple alongside Google only when all five settings exist:

- `APPLE_APP_ID`: `org.freevia.qcremote`, the primary Apple App ID.
- `APPLE_CLIENT_ID`: `org.freevia.qcremote.web`, the web Services ID.
- `APPLE_TEAM_ID`: the distribution team's identifier.
- `APPLE_KEY_ID`: the key restricted to Sign in with Apple for this primary app.
- `APPLE_PRIVATE_KEY`: the downloaded P-256 private key, kept in a Worker secret.

Enable Sign in with Apple for the primary App ID. Associate the web Services ID
with the primary app and `qcrc.freevia.org`, with these exact return URLs:

- `https://qcrc.freevia.org/auth/apple/pair/callback`
- `https://qcrc.freevia.org/auth/apple/mcp/callback`
- `https://qcrc.freevia.org/auth/apple/delete/callback`

Register `https://qcrc.freevia.org/auth/apple/notifications` as the primary app's
server-to-server notification endpoint. Do not print keys, pass them as command
arguments, put them in source, or expose them in browser logs. Use file input to
Wrangler's secret commands. Retain existing Google and cookie secrets unchanged.
Deploy the reviewed cloud change before exposing Apple sign-in to users.

Apple subjects are namespaced separately from existing Google subjects. Matching
email addresses never merge accounts. Pair the host and controller using the
same account and provider. Hosts must include the shared relay client's Apple
identity-provider support; old Google-only hosts cannot redeem an Apple receipt.

Account deletion obtains a fresh Apple access token, holds it encrypted inside
the short-lived signed deletion confirmation, and revokes it only after the
existing typed DELETE confirmation. Signed revocation and account-deletion
notifications establish the existing account-deletion fence. Persistent,
non-identifying event timestamps reject replays and events older than a new
sign-in. Existing Google OAuth state, subject IDs, grants, and credentials retain
their prior meaning and do not require a migration or a new sign-in.

## Validation and submission

1. Run iOS tests/typecheck, cloud tests/check, contract checks, and shared relay
   Rust tests. Review the auth and account-deletion changes before deployment.
2. Archive from the release commit using the real distribution team. Verify the
   archive has no `.dext` and no DriverKit entitlement, and includes the native
   bridge privacy manifest. Export using `app-store-connect` and validate the IPA.
3. Verify Google saved-session recovery and new Apple pairing on a compatible
   physical host and controller; never use a real preset mutation for a smoke
   test. Exercise Apple cancellation, private email, sign-out and account deletion
   with dedicated test accounts rather than the user's existing paired account.
4. Test on physical iPhone and iPad, and record hardware interaction evidence
   required by Apple's review guidance. Simulator results are not physical proof.
5. Complete accurate App Privacy/age-rating declarations, support and privacy
   URLs, screenshots, review contact details, and review access instructions.
   Label demo screenshots and explain the running paired-host/USB requirements.
6. Upload through App Store Connect/TestFlight. Do not claim submission or
   publication until Apple shows the corresponding state.

Apple requirements: https://developer.apple.com/app-store/review/guidelines/
Setup: https://developer.apple.com/help/account/capabilities/configure-sign-in-with-apple-for-the-web/
Deletion: https://developer.apple.com/documentation/technotes/tn3194-handling-account-deletions-and-revoking-tokens-for-sign-in-with-apple
