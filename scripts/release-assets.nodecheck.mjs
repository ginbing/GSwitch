import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile, copyFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";

const script = resolve("scripts/release-assets.mjs");
const version = JSON.parse(await readFile("src-tauri/tauri.conf.json", "utf8")).version;
const base = `GSwitch_${version}_`;
const signature = Buffer.from("untrusted comment: test signature\ntrusted comment: version:test\n").toString("base64");
const platformFiles = {
  windows: [["src-tauri/target/release/bundle/nsis", `${base}x64-setup.exe`]],
  "macos-silicon": [
    ["src-tauri/target/aarch64-apple-darwin/release/bundle/dmg", `${base}aarch64.dmg`],
    ["src-tauri/target/aarch64-apple-darwin/release/bundle/macos", `${base}aarch64.app.tar.gz`],
  ],
  "macos-intel": [
    ["src-tauri/target/x86_64-apple-darwin/release/bundle/dmg", `${base}x64.dmg`],
    ["src-tauri/target/x86_64-apple-darwin/release/bundle/macos", `${base}x64.app.tar.gz`],
  ],
  linux: [
    ["src-tauri/target/release/bundle/appimage", `${base}amd64.AppImage`],
    ["src-tauri/target/release/bundle/deb", `${base}amd64.deb`],
  ],
};

function run(cwd, ...args) {
  return spawnSync(process.execPath, [script, ...args], { cwd, encoding: "utf8" });
}

test("stages exact platform bytes and rejects changed release assets", async () => {
  const root = await mkdtemp(join(tmpdir(), "gswitch-release-assets-"));
  assert(root.startsWith(join(tmpdir(), "gswitch-release-assets-")));
  try {
    await mkdir(join(root, "src-tauri"), { recursive: true });
    await writeFile(join(root, "src-tauri/tauri.conf.json"), JSON.stringify({ version }));
    const release = join(root, "release-assets");
    await mkdir(release);
    for (const [platform, files] of Object.entries(platformFiles)) {
      const staging = join(root, `stage-${platform}`);
      for (const [folder, file] of files) {
        const path = join(root, folder, file);
        await mkdir(dirname(path), { recursive: true });
        await writeFile(path, `final bytes for ${file}`);
        if (!file.endsWith(".dmg")) await writeFile(`${path}.sig`, signature);
      }
      const result = run(root, "stage", platform, staging);
      assert.equal(result.status, 0, result.stderr);
      for (const file of await readdir(staging)) await copyFile(join(staging, file), join(release, file));
    }
    const components = [
      { purl: "pkg:npm/%40tauri-apps/api@2.11.1" },
      { purl: "pkg:cargo/tauri@2.11.6" },
      ...Array.from({ length: 100 }, (_, i) => ({ purl: `pkg:npm/test-${i}@1` })),
      ...Array.from({ length: 300 }, (_, i) => ({ purl: `pkg:cargo/test-${i}@1` })),
    ];
    await writeFile(join(release, `${base}sbom.cdx.json`), JSON.stringify({ bomFormat: "CycloneDX", components }));
    const tag = `v${version}`;
    const assembled = run(root, "assemble", release, tag, "ginbing/GSwitch");
    assert.equal(assembled.status, 0, assembled.stderr);
    const manifest = JSON.parse(await readFile(join(release, "latest.json"), "utf8"));
    assert.equal(manifest.version, version);
    assert.equal(Object.keys(manifest.platforms).length, 9);
    assert.equal(run(root, "verify", release, tag, "ginbing/GSwitch").status, 0);
    await writeFile(join(release, `${base}x64-setup.exe`), "changed after signing");
    const changed = run(root, "verify", release, tag, "ginbing/GSwitch");
    assert.notEqual(changed.status, 0);
    assert.match(changed.stderr, /Digest mismatch/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
