import { readFile, writeFile } from "node:fs/promises";

const paths = {
  tauri: new URL("../src-tauri/tauri.conf.json", import.meta.url),
  package: new URL("../package.json", import.meta.url),
  cargo: new URL("../src-tauri/Cargo.toml", import.meta.url),
  lock: new URL("../src-tauri/Cargo.lock", import.meta.url),
};
const checkOnly = process.argv.includes("--check");

function replaceVersion(content, expression, version, label) {
  if (!expression.test(content)) {
    throw new Error(`Unable to locate the ${label} version.`);
  }
  return content.replace(expression, `$1${version}$2`);
}

const tauri = JSON.parse(await readFile(paths.tauri, "utf8"));
const version = tauri.version;
if (typeof version !== "string" || !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
  throw new Error("src-tauri/tauri.conf.json must contain a SemVer version.");
}

const packageJson = JSON.stringify(
  { ...JSON.parse(await readFile(paths.package, "utf8")), version },
  null,
  2,
) + "\n";
const cargoToml = replaceVersion(
  await readFile(paths.cargo, "utf8"),
  /(^\[package\][\s\S]*?^version = ")[^"]+("\r?$)/m,
  version,
  "Cargo package",
);
const cargoLock = replaceVersion(
  await readFile(paths.lock, "utf8"),
  /(^\[\[package\]\]\r?\nname = "gswitch"\r?\nversion = ")[^"]+("\r?$)/m,
  version,
  "Cargo lock package",
);

const expected = [
  [paths.package, packageJson, "package.json"],
  [paths.cargo, cargoToml, "src-tauri/Cargo.toml"],
  [paths.lock, cargoLock, "src-tauri/Cargo.lock"],
];

for (const [path, content, label] of expected) {
  const current = await readFile(path, "utf8");
  if (current === content) {
    continue;
  }
  if (checkOnly) {
    throw new Error(`${label} is not synchronized with src-tauri/tauri.conf.json.`);
  }
  await writeFile(path, content);
}

console.log(`${checkOnly ? "Verified" : "Synchronized"} version ${version}.`);
