# Keeping GitHub and the website in sync

The marketing page is <https://freevia.org/qcremote/>. Its Windows and Android
screenshots are the canonical presentation captures; the README embeds those
same live image URLs instead of maintaining duplicate image files. The user
guide intentionally uses separate demo-mode screenshots for instruction.

When changing a release, screenshot, supported platform, AI connection, safety
claim, or public policy, update the website and the relevant repository page
together. Publish the website before merging a README change that depends on
new website assets or links. Check captures for names, accounts, serial numbers,
notifications, and other personal details before publishing them.

Run `node scripts/check-website-sync.mjs` to compare the public page and README.
GitHub Actions runs the same check on README changes and daily. It verifies the
two screenshot URLs, Windows, Android and Linux preview download links, stable
and Linux preview release notes, and
Google Play link. The check flags drift; it does not deploy the website or
replace a human review of feature descriptions and legal text.
