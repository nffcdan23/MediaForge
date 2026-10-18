#!/usr/bin/env node
// Downloads the third-party tools listed in packaging/tools.lock, checks each against its pinned SHA-256, and
// lays them out under tools/<platform>/ the way packaging/bundle.<platform>.json expects.
//
//   node scripts/fetch-tools.mjs macos|windows [--cache <folder>] [--download-only]
//
// --download-only stops after downloading and checking, before laying anything out (the macOS FFmpeg build
// needs the sources first).
//
// Nothing is installed on the system and no downloaded program is run, except `--version` checks at the end.
// --cache (or MEDIAFORGE_TOOLS_CACHE) names a folder of already-downloaded files to use when their checksum
// matches, instead of downloading them again.

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, renameSync, rmSync, statSync, writeFileSync, chmodSync, cpSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const platform = process.argv[2];
if (!["macos", "windows"].includes(platform)) {
  console.error("Usage: node scripts/fetch-tools.mjs macos|windows [--cache <folder>]");
  process.exit(2);
}
const cacheIndex = process.argv.indexOf("--cache");
const cache = cacheIndex > 0 ? process.argv[cacheIndex + 1] : process.env.MEDIAFORGE_TOOLS_CACHE;

const downloads = join(root, "tools", "downloads");
const out = join(root, "tools", platform);
mkdirSync(downloads, { recursive: true });

const entries = readFileSync(join(root, "packaging", "tools.lock"), "utf8")
  .split("\n")
  .map((line) => line.trim())
  .filter((line) => line && !line.startsWith("#"))
  .map((line) => {
    const [target, name, version, sha, url] = line.split(/\s+/);
    return { target, name, version, sha, url };
  })
  .filter((entry) => entry.target === platform || entry.target === "all");

const sha256 = (path) => createHash("sha256").update(readFileSync(path)).digest("hex");
const verified = (path, sha) => existsSync(path) && (sha === "-" || sha256(path) === sha);

async function download(entry) {
  const target = join(downloads, entry.name);
  if (verified(target, entry.sha)) return console.log(`  ok        ${entry.name.padEnd(56)} ${entry.version}`);
  if (cache && verified(join(cache, entry.name), entry.sha)) {
    copyFileSync(join(cache, entry.name), target);
    return console.log(`  cached    ${entry.name.padEnd(56)} ${entry.version}`);
  }
  if (!entry.url.startsWith("https://")) throw new Error(`${entry.name}: only https downloads are allowed`);
  console.log(`  fetching  ${entry.name.padEnd(56)} ${entry.version}`);
  let response;
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    response = await fetch(entry.url, { redirect: "follow" }).catch((error) => ({ ok: false, statusText: String(error) }));
    if (response.ok) break;
    await new Promise((resolve) => setTimeout(resolve, 2000 * attempt));
  }
  if (!response.ok) throw new Error(`${entry.name}: download failed (${response.status ?? ""} ${response.statusText})`);
  if (response.url && !response.url.startsWith("https://")) throw new Error(`${entry.name}: redirected away from https`);
  const partial = `${target}.partial`;
  writeFileSync(partial, Buffer.from(await response.arrayBuffer()));
  if (entry.sha !== "-" && sha256(partial) !== entry.sha) {
    renameSync(partial, `${target}.rejected`);
    throw new Error(`${entry.name}: checksum mismatch (expected ${entry.sha})`);
  }
  renameSync(partial, target);
}

function extract(archive, into) {
  rmSync(into, { recursive: true, force: true });
  mkdirSync(into, { recursive: true });
  if (process.platform === "darwin") execFileSync("ditto", ["-x", "-k", archive, into]);
  else execFileSync("tar", ["-xf", archive, "-C", into]); // bsdtar, part of Windows 10 and 11
}

const fresh = (path) => {
  rmSync(path, { recursive: true, force: true });
  mkdirSync(path, { recursive: true });
  return path;
};

const version = (name) => entries.find((e) => e.name.startsWith(name))?.version ?? "?";

function notices(folder, template, values, extra) {
  fresh(folder);
  let text = readFileSync(join(root, "packaging", template), "utf8");
  for (const [key, value] of Object.entries(values)) text = text.replaceAll(`@${key}@`, value);
  writeFileSync(join(folder, "THIRD-PARTY-NOTICES.txt"), text);
  for (const name of ["yt-dlp-LICENSE.txt", "yt-dlp-THIRD_PARTY_LICENSES.txt", "deno-LICENSE.md"]) {
    copyFileSync(join(downloads, name), join(folder, name));
  }
  copyFileSync(join(root, "packaging", "licenses", "GPL-3.0.txt"), join(folder, "GPL-3.0.txt"));
  for (const [source, name] of extra) copyFileSync(source, join(folder, name));
}

function assembleMacos() {
  const bin = fresh(join(out, "bin"));
  const scratch = fresh(join(out, ".scratch"));

  // yt-dlp: the project's self-contained package, unmodified apart from the program file's name.
  const ytdlp = join(out, "yt-dlp");
  extract(join(downloads, "yt-dlp_macos.zip"), ytdlp);
  renameSync(join(ytdlp, "yt-dlp_macos"), join(ytdlp, "yt-dlp"));
  chmodSync(join(ytdlp, "yt-dlp"), 0o755);

  // Deno: the publisher's two builds joined into one universal file; each half keeps Deno Land's signature.
  extract(join(downloads, "deno-aarch64-apple-darwin.zip"), join(scratch, "arm64"));
  extract(join(downloads, "deno-x86_64-apple-darwin.zip"), join(scratch, "x86_64"));
  execFileSync("lipo", ["-create", join(scratch, "arm64", "deno"), join(scratch, "x86_64", "deno"), "-output", join(bin, "deno-universal-apple-darwin")]);

  // FFmpeg and FFprobe: our own LGPL build (scripts/build-ffmpeg-macos.sh).
  const built = join(out, "ffmpeg-build");
  for (const tool of ["ffmpeg", "ffprobe"]) {
    if (!existsSync(join(built, tool))) throw new Error(`Missing ${tool}. Run scripts/build-ffmpeg-macos.sh first.`);
    copyFileSync(join(built, tool), join(bin, `${tool}-universal-apple-darwin`));
  }
  // A universal build compiles each architecture on its own first, and Tauri checks for that architecture's
  // copy of each helper as it does; the universal copies are the ones that end up in the app.
  for (const [arch, triple] of [["arm64", "aarch64-apple-darwin"], ["x86_64", "x86_64-apple-darwin"]]) {
    copyFileSync(join(scratch, arch, "deno"), join(bin, `deno-${triple}`));
    for (const tool of ["ffmpeg", "ffprobe"]) {
      execFileSync("lipo", [join(built, tool), "-thin", arch, "-output", join(bin, `${tool}-${triple}`)]);
    }
  }
  for (const file of readdirSync(bin)) chmodSync(join(bin, file), 0o755);

  const licences = join(built, "licenses");
  notices(
    join(out, "notices"),
    "THIRD-PARTY-NOTICES.macos.txt",
    { FFMPEG_VERSION: version("ffmpeg-"), LAME_VERSION: version("lame-"), YTDLP_VERSION: version("yt-dlp_macos"), DENO_VERSION: version("deno-aarch64") },
    readdirSync(licences).map((name) => [join(licences, name), name]),
  );
  rmSync(scratch, { recursive: true, force: true });
}

function assembleWindows() {
  const bin = fresh(join(out, "bin"));
  const scratch = fresh(join(out, ".scratch"));

  // yt-dlp: the project's folder build (a program plus its Python runtime), so nothing is unpacked to a
  // temporary folder each time it runs, which also keeps antivirus scanners calmer than the one-file build.
  const ytdlp = join(out, "yt-dlp");
  extract(join(downloads, "yt-dlp_win.zip"), ytdlp);
  if (!existsSync(join(ytdlp, "yt-dlp.exe"))) {
    // Some releases wrap the folder in another one.
    const inner = readdirSync(ytdlp).map((n) => join(ytdlp, n)).find((p) => existsSync(join(p, "yt-dlp.exe")));
    if (!inner) throw new Error("yt-dlp.exe not found in yt-dlp_win.zip");
    cpSync(inner, join(scratch, "yt-dlp"), { recursive: true });
    rmSync(ytdlp, { recursive: true, force: true });
    renameSync(join(scratch, "yt-dlp"), ytdlp);
  }

  // FFmpeg: the LGPL build with shared libraries. The programs and the DLLs they load sit together.
  const ffmpegZip = entries.find((e) => e.name.includes("win64-lgpl-shared")).name;
  extract(join(downloads, ffmpegZip), join(scratch, "ffmpeg"));
  const top = join(scratch, "ffmpeg", readdirSync(join(scratch, "ffmpeg"))[0]);
  const ffmpeg = fresh(join(out, "ffmpeg"));
  for (const file of readdirSync(join(top, "bin"))) {
    if (file.toLowerCase() === "ffplay.exe") continue;
    copyFileSync(join(top, "bin", file), join(ffmpeg, file));
  }
  // Run on Windows, where this layout is assembled, to confirm the build's licence from its own configuration.
  const configuration = execFileSync(join(ffmpeg, "ffmpeg.exe"), ["-hide_banner", "-version"], { encoding: "utf8" });
  if (/--enable-gpl|--enable-nonfree/.test(configuration)) throw new Error("The FFmpeg build is not LGPL.");
  writeFileSync(join(scratch, "FFmpeg-BUILD-CONFIGURATION.txt"), configuration);

  // Deno: the publisher's build.
  extract(join(downloads, "deno-x86_64-pc-windows-msvc.zip"), join(scratch, "deno"));
  copyFileSync(join(scratch, "deno", "deno.exe"), join(bin, "deno-x86_64-pc-windows-msvc.exe"));

  notices(
    join(out, "notices"),
    "THIRD-PARTY-NOTICES.windows.txt",
    { FFMPEG_VERSION: version("ffmpeg-n"), FFMPEG_ARCHIVE: ffmpegZip, YTDLP_VERSION: version("yt-dlp_win"), DENO_VERSION: version("deno-x86_64-pc") },
    [
      [join(top, "LICENSE.txt"), "FFmpeg-LICENSE.txt"],
      [join(scratch, "FFmpeg-BUILD-CONFIGURATION.txt"), "FFmpeg-BUILD-CONFIGURATION.txt"],
    ],
  );
  rmSync(scratch, { recursive: true, force: true });
}

function report() {
  const run = (file, args) => {
    try {
      return execFileSync(file, args, { encoding: "utf8", cwd: out, stdio: ["ignore", "pipe", "ignore"] }).split("\n")[0].trim();
    } catch {
      return "(did not run on this computer)";
    }
  };
  const size = (path) => {
    const walk = (p) => (statSync(p).isDirectory() ? readdirSync(p).reduce((sum, n) => sum + walk(join(p, n)), 0) : statSync(p).size);
    return `${Math.round(walk(path) / 1e6)} MB`;
  };
  if (platform === "macos") {
    console.log(`  yt-dlp    ${run(join(out, "yt-dlp", "yt-dlp"), ["--version"])}  ${size(join(out, "yt-dlp"))}`);
    console.log(`  ffmpeg    ${run(join(out, "bin", "ffmpeg-universal-apple-darwin"), ["-hide_banner", "-version"])}`);
    console.log(`  deno      ${run(join(out, "bin", "deno-universal-apple-darwin"), ["--version"])}`);
  } else {
    console.log(`  yt-dlp    ${run(join(out, "yt-dlp", "yt-dlp.exe"), ["--version"])}  ${size(join(out, "yt-dlp"))}`);
    console.log(`  ffmpeg    ${run(join(out, "ffmpeg", "ffmpeg.exe"), ["-hide_banner", "-version"])}  ${size(join(out, "ffmpeg"))}`);
    console.log(`  deno      ${run(join(out, "bin", "deno-x86_64-pc-windows-msvc.exe"), ["--version"])}`);
  }
}

for (const entry of entries) await download(entry);
console.log("All components present and verified in tools/downloads.");
if (process.argv.includes("--download-only")) process.exit(0);
console.log(`Assembling tools/${platform}…`);
if (platform === "macos") assembleMacos();
else assembleWindows();
report();
console.log(`Ready: tools/${platform}`);
