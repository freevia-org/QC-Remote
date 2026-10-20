import assert from "node:assert/strict";
import test from "node:test";
import { applicationModifier, hostPlatform } from "../src/host-platform.ts";
import { buildIssueUrl } from "../src/about-content.ts";

test("Mac webviews without userAgentData identify macOS and accept Command shortcuts", () => {
  assert.equal(hostPlatform({platform:"MacIntel"}), "macOS");
  assert.equal(applicationModifier({ctrlKey:false, metaKey:true}, "macOS"), true);
  assert.equal(applicationModifier({ctrlKey:false, metaKey:true}, "Windows"), false);
  assert.equal(applicationModifier({ctrlKey:true, metaKey:false}, "macOS"), true);
});
test("issue forms label the actual host and leave unknown platforms unlabeled", () => {
  for (const platform of ["macOS", "Windows", "Linux", undefined]) {
    const url = new URL(buildIssueUrl({repositoryUrl:"https://github.com/freevia-org/QC-Remote",kind:"bug",appVersion:"1.2.3",platform}));
    const version = platform ? `${platform} 1.2.3` : "1.2.3";
    assert.equal(url.searchParams.get("app-version"), version);
    assert.equal(url.searchParams.get("title"), `[Bug] ${version}: `);
    assert.ok(url.searchParams.get("behavior")?.includes(`App version: ${version}`));
  }
});
