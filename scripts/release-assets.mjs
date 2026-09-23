import { spawnSync } from "node:child_process";
import { copyFile, mkdir, readFile, readdir, stat, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";

const version = JSON.parse(await readFile("src-tauri/tauri.conf.json", "utf8")).version;
const name = (suffix) => `GSwitch_${version}_${suffix}`;
const targets = {
  windows: {
    base: "src-tauri/target/release/bundle",
    files: [["nsis", name("x64-setup.exe")]],
  },
  "macos-silicon": {
    base: "src-tauri/target/aarch64-apple-darwin/release/bundle",
    files: [["dmg", name("aarch64.dmg")], ["macos", name("aarch64.app.tar.gz"), "GSwitch.app.tar.gz"]],
  },
  "macos-intel": {
    base: "src-tauri/target/x86_64-apple-darwin/release/bundle",
    files: [["dmg", name("x64.dmg")], ["macos", name("x64.app.tar.gz"), "GSwitch.app.tar.gz"]],
  },
  linux: {
    base: "src-tauri/target/release/bundle",
    files: [["appimage", name("amd64.AppImage")], ["deb", name("amd64.deb")]],
  },
};
const packageFiles = Object.values(targets).flatMap(({ files }) => files.map(([, file]) => file));
const signedFiles = packageFiles.filter((file) => !file.endsWith(".dmg"));
const stagedFiles = [...packageFiles, ...signedFiles.map((file) => `${file}.sig`)];
const sbomFile = `GSwitch_${version}_sbom.cdx.json`;
const finalFiles = [...stagedFiles, sbomFile, "latest.json"];

function requireCondition(value, message) {
  if (!value) throw new Error(message);
}

async function requireFile(path) {
  const info = await stat(path);
  requireCondition(info.isFile() && info.size > 0, `Missing or empty release asset: ${path}`);
}

async function stage(id, destination) {
  const target = targets[id];
  requireCondition(target, `Unknown release platform: ${id}`);
  await mkdir(destination, { recursive: true });
  const files = target.files.flatMap(([folder, file, sourceName = file]) => [
    [join(target.base, folder, sourceName), file],
    ...(!file.endsWith(".dmg") ? [[join(target.base, folder, `${sourceName}.sig`), `${file}.sig`]] : []),
  ]);
  for (const [source, file] of files) {
    await requireFile(source);
    const output = join(destination, file);
    await copyFile(source, output);
  }
  console.log(`Staged ${id}: ${files.map(([, file]) => file).join(", ")}`);
}

async function sign(id) {
  const target = targets[id];
  requireCondition(target, `Unknown release platform: ${id}`);
  requireCondition(process.env.TAURI_SIGNING_PRIVATE_KEY, "Missing updater signing key");
  for (const [folder, file, sourceName = file] of target.files) {
    if (file.endsWith(".dmg")) continue;
    const source = join(target.base, folder, sourceName);
    await requireFile(source);
    const signerArgs = [
      resolve("node_modules/@tauri-apps/cli/tauri.js"), "signer", "sign",
      "--app-version", version,
    ];
    if (!process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD) signerArgs.push("--password", "");
    signerArgs.push(source);
    const signed = spawnSync(process.execPath, signerArgs, {
      stdio: "inherit",
    });
    requireCondition(signed.status === 0, `Could not sign final bytes of ${file}`);
    await signature(".", source);
  }
}

async function signature(directory, file) {
  const encoded = (await readFile(join(directory, `${file}.sig`), "utf8")).trim();
  requireCondition(/^[A-Za-z0-9+/]+={0,2}$/.test(encoded), `Invalid signature encoding for ${file}`);
  const decoded = Buffer.from(encoded, "base64").toString("utf8");
  requireCondition(decoded.includes("untrusted comment:") && decoded.includes("trusted comment:"), `Invalid Tauri signature for ${file}`);
  const trusted = decoded.split(/\r?\n/).find((line) => line.startsWith("trusted comment:"));
  requireCondition(trusted?.split("\t").includes(`version:${version}`), `Signature version does not match ${version} for ${file}`);
  return encoded;
}

async function validateSbom(directory) {
  const sbom = JSON.parse(await readFile(join(directory, sbomFile), "utf8"));
  requireCondition(sbom.bomFormat === "CycloneDX", "SBOM must be CycloneDX");
  const purls = (sbom.components ?? []).map((component) => component.purl ?? "");
  requireCondition(purls.filter((purl) => purl.startsWith("pkg:npm/")).length > 100, "SBOM lacks the pnpm graph");
  requireCondition(purls.filter((purl) => purl.startsWith("pkg:cargo/")).length > 300, "SBOM lacks the Cargo graph");
  requireCondition(purls.some((purl) => purl.startsWith("pkg:npm/%40tauri-apps/api@")), "SBOM lacks Tauri JavaScript");
  requireCondition(purls.some((purl) => purl.startsWith("pkg:cargo/tauri@")), "SBOM lacks Tauri Rust");
}

function releaseUrl(repository, tag, file) {
  return `https://github.com/${repository}/releases/download/${tag}/${encodeURIComponent(file)}`;
}

async function expectedPlatforms(directory, repository, tag) {
  const entries = [
    ["windows-x86_64", name("x64-setup.exe")],
    ["windows-x86_64-nsis", name("x64-setup.exe")],
    ["darwin-aarch64", name("aarch64.app.tar.gz")],
    ["darwin-aarch64-app", name("aarch64.app.tar.gz")],
    ["darwin-x86_64", name("x64.app.tar.gz")],
    ["darwin-x86_64-app", name("x64.app.tar.gz")],
    ["linux-x86_64", name("amd64.AppImage")],
    ["linux-x86_64-appimage", name("amd64.AppImage")],
    ["linux-x86_64-deb", name("amd64.deb")],
  ];
  return Object.fromEntries(await Promise.all(entries.map(async ([platform, file]) => [platform, {
    signature: await signature(directory, file),
    url: releaseUrl(repository, tag, file),
  }])));
}

async function requireExactFiles(directory, expected) {
  const actual = (await readdir(directory)).sort();
  const wanted = [...expected].sort();
  requireCondition(JSON.stringify(actual) === JSON.stringify(wanted), `Unexpected release asset set: ${actual.join(", ")}`);
  for (const file of wanted) await requireFile(join(directory, file));
}

async function assemble(directory, tag, repository) {
  requireCondition(tag === `v${version}`, `Release tag ${tag} does not match ${version}`);
  requireCondition(/^[\w.-]+\/[\w.-]+$/.test(repository), "Invalid GitHub repository");
  await requireExactFiles(directory, [...stagedFiles, sbomFile]);
  await validateSbom(directory);
  const platforms = await expectedPlatforms(directory, repository, tag);
  const latest = { version, notes: "", pub_date: new Date().toISOString(), platforms };
  await writeFile(join(directory, "latest.json"), `${JSON.stringify(latest, null, 2)}\n`);
  await verify(directory, tag, repository);
  console.log(`Assembled ${finalFiles.length} final assets for ${tag}`);
}

async function verify(directory, tag, repository) {
  requireCondition(tag === `v${version}`, `Release tag ${tag} does not match ${version}`);
  await requireExactFiles(directory, finalFiles);
  await validateSbom(directory);
  const latest = JSON.parse(await readFile(join(directory, "latest.json"), "utf8"));
  requireCondition(latest.version === version && !Number.isNaN(Date.parse(latest.pub_date)), "Invalid updater version or publication date");
  const platforms = await expectedPlatforms(directory, repository, tag);
  requireCondition(JSON.stringify(latest.platforms) === JSON.stringify(platforms), "Updater manifest does not match final assets");
  console.log(`Verified ${tag} release asset set and updater manifest`);
}

const [command, path, tag, repository] = process.argv.slice(2);
requireCondition(path, "Usage: release-assets.mjs <stage|assemble|verify> <directory|platform> ...");
if (command === "stage") await stage(path, resolve(tag));
else if (command === "sign") await sign(path);
else if (command === "assemble") await assemble(resolve(path), tag, repository);
else if (command === "verify") await verify(resolve(path), tag, repository);
else throw new Error(`Unknown release asset command: ${command}`);
