import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "../..");
const readJson = (path) => JSON.parse(readFileSync(join(root, path), "utf8"));
const pkg = readJson("package.json");
const npmLock = readJson("package-lock.json");
const tauri = readJson("src-tauri/tauri.conf.json");
const cargo = readFileSync(join(root, "src-tauri/Cargo.toml"), "utf8");
const cargoLock = readFileSync(join(root, "src-tauri/Cargo.lock"), "utf8");

function packageVersion(toml, name) {
  const packages = [...toml.matchAll(/(?:^|\n)\[\[package\]\]\s*\n([\s\S]*?)(?=\n\[\[package\]\]|$)/g)];
  const entry = packages.find(([, body]) => /^name\s*=\s*"aurora-launcher"\s*$/m.test(body));
  if (name === "Cargo.lock") {
    if (!entry) throw new Error("Cargo.lock is missing the aurora-launcher package");
    return /^version\s*=\s*"([^"]+)"\s*$/m.exec(entry[1])?.[1];
  }
  return /^\[package\]\s*\n[\s\S]*?^version\s*=\s*"([^"]+)"\s*$/m.exec(toml)?.[1];
}

const version = pkg.version;
if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version ?? "")) {
  throw new Error("Launcher version in package.json must be numeric major.minor.patch for Windows installers");
}
const [major, minor, patch] = version.split(".").map(Number);
if (major > 255 || minor > 255 || patch > 65535) {
  throw new Error("Launcher version exceeds Windows MSI ProductVersion limits");
}
const expected = {
  "package-lock.json": npmLock.version,
  "package-lock.json root": npmLock.packages?.[""]?.version,
  "Cargo.toml": packageVersion(cargo, "Cargo.toml"),
  "Cargo.lock": packageVersion(cargoLock, "Cargo.lock"),
};
for (const [file, actual] of Object.entries(expected)) {
  if (actual !== version) throw new Error(`${file} version ${actual ?? "missing"} differs from package.json ${version}`);
}
if (tauri.version !== "../package.json") {
  throw new Error("Tauri version must resolve from the authoritative package.json");
}

if (process.argv.length > 2) {
  if (process.argv.length !== 4 || process.argv[2] !== "--release") {
    throw new Error("Usage: node tools/release/verify-version.mjs [--release <version>]");
  }
  const requested = process.argv[3];
  if (requested !== version) throw new Error(`Requested launcher version ${requested} differs from source ${version}`);
  if (version === "0.1.0") {
    throw new Error("0.1.0 is the inherited development version; choose and synchronize an intentional first production version before publication");
  }
}

console.log(`Aurora Launcher version contract: ${version}`);
