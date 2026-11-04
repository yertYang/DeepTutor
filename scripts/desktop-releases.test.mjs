import { test } from "node:test";
import assert from "node:assert/strict";
import { desktopReleases } from "../web/lib/desktop-releases.ts";
const repo = "https://github.com/yertYang/Lorevia";
const asset = (name, more = {}) => ({
  name,
  size: 100,
  state: "uploaded",
  browser_download_url: repo + "/releases/download/preview/" + name,
  ...more,
});
const release = (assets, more = {}) => ({
  name: "Desktop preview",
  tag_name: "preview",
  html_url: repo + "/releases/tag/preview",
  draft: false,
  prerelease: true,
  assets,
  ...more,
});
test("empty releases and source-only releases do not pretend an installer exists", () => {
  assert.deepEqual(desktopReleases([], repo), []);
  assert.deepEqual(
    desktopReleases([release([asset("source.tar.gz")])], repo),
    [],
  );
  assert.throws(() => desktopReleases({}, repo));
});
test("published native platform names match the actual packaging targets", () => {
  const result = desktopReleases(
    [
      release(
        [
          "Lorevia-MacOS-Apple-Silicon-v1.7.1.dmg",
          "Lorevia-MacOS-Intel-v1.7.1.dmg",
          "Lorevia-Windows-v1.7.1-Setup.exe",
          "Lorevia-Windows-v1.7.1-Portable.zip",
        ].map((x) => asset(x)),
      ),
    ],
    repo,
  );
  assert.deepEqual(
    result[0].assets.map((x) => x.platform),
    ["macos-arm64", "macos-x64", "windows-x64", "windows-x64"],
  );
  assert.equal(result[0].preview, true);
});
test("drafts, incomplete uploads, empty files and forged external URLs cannot become download links", () => {
  const good = asset("Lorevia-MacOS-Apple-Silicon-v1.7.1.dmg");
  assert.deepEqual(
    desktopReleases(
      [
        release([good], { draft: true }),
        release([good], { html_url: "https://evil.example/releases/tag/x" }),
      ],
      repo,
    ),
    [],
  );
  for (const more of [
    { state: "new" },
    { size: 0 },
    { size: -1 },
    { size: 1.5 },
    { browser_download_url: "javascript:alert(1)" },
    { browser_download_url: "https://evil.example/Lorevia.dmg" },
    {
      browser_download_url:
        "https://github.com/other/Lorevia/releases/download/x/a.dmg",
    },
    { browser_download_url: repo + "/releases/download/../../issues" },
  ]) {
    assert.deepEqual(
      desktopReleases([release([{ ...good, ...more }])], repo),
      [],
    );
  }
});
test("release order and stable/preview distinction survive partial platform availability", () => {
  const rows = desktopReleases(
    [
      release([asset("Lorevia-MacOS-Apple-Silicon-v1.7.1.dmg")]),
      release([asset("Lorevia-Windows-v1.7.0-Setup.exe")], {
        name: "Stable",
        prerelease: false,
      }),
    ],
    repo,
  );
  assert.equal(rows.length, 2);
  assert.equal(rows[0].preview, true);
  assert.equal(rows[1].preview, false);
  assert.equal(
    rows[0].assets.some((a) => a.platform === "windows-x64"),
    false,
  );
});
