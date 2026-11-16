#!/usr/bin/env node
/**
 * Cabinet brand assets (#268).
 *
 * Rasterizes the drawn Ludex mark into the platform icons from one SVG, in the
 * Cabinet palette: deep warm charcoal, paper ink, one signal full stop. The
 * drawing is unchanged — the same L and dot the wordmark has always used — only
 * the material moves, so the app, the Tauri window, the PWA and the Sunshine
 * poster read as one identity.
 *
 *   node e2e/cabinet-assets.mjs
 *
 * Writes:
 *   apps/web/public/app-icon.svg, favicon.svg
 *   apps/web/public/icons/icon-{192,512}.png, maskable-512.png, apple-touch-icon.png
 *   apps/desktop/src-tauri/icons/icon.png, 32x32.png, 64x64.png, 128x128.png,
 *     128x128@2x.png, icon.ico, icon.icns, Square*Logo.png, StoreLogo.png
 *   apps/desktop/src-tauri/assets/ludex-console.png   (Sunshine app artwork)
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import sharp from "sharp";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const WEB = join(ROOT, "apps", "web", "public");
const TAURI = join(ROOT, "apps", "desktop", "src-tauri");

/** The Cabinet palette, mirrored from theme/console-room.ts. */
const CABINET = {
  deep: "#070706",
  charcoal: "#0d0c0a",
  ink: "#f3eee2",
  signal: "#ffc61a",
};

/** The app icon: the drawn L and its dot on the deep background. */
function markSvg({ size = 512, maskable = false } = {}) {
  // A maskable icon keeps 20 % of padding so a round or squircle launcher mask
  // never crops the letter or the dot.
  const scale = maskable ? 0.78 : 1;
  const translate = (512 - 512 * scale) / 2;
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="${size}" height="${size}">
  <rect width="512" height="512" fill="${CABINET.deep}"/>
  <g transform="translate(${translate + 106 * scale} ${translate + 88 * scale}) scale(${3.1 * scale})">
    <path d="M0 0H22V78H66V100H0Z" fill="${CABINET.ink}"/>
  </g>
  <rect x="${translate + 330 * scale}" y="${translate + 300 * scale}" width="${104 * scale}" height="${104 * scale}" rx="${2 * scale}" fill="${CABINET.signal}"/>
</svg>`;
}

/** The favicon: the same drawing at 64, with a 12-unit dot. */
const faviconSvg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64">
  <rect width="64" height="64" fill="${CABINET.deep}"/>
  <g transform="translate(13 11) scale(0.4)">
    <path d="M0 0H22V78H66V100H0Z" fill="${CABINET.ink}"/>
  </g>
  <rect x="42" y="38" width="13" height="13" fill="${CABINET.signal}"/>
</svg>`;

/** Sunshine uses a 2:3 poster: the mark, the wordmark, one rule. */
const posterSvg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 600 900" width="600" height="900">
  <rect width="600" height="900" fill="${CABINET.charcoal}"/>
  <g transform="translate(150 200) scale(2.2)">
    <path d="M0 0H22V78H66V100H0Z" fill="${CABINET.ink}"/>
  </g>
  <rect x="330" y="380" width="80" height="80" fill="${CABINET.signal}"/>
  <g transform="translate(120 690) scale(0.72)" fill="${CABINET.ink}">
    <path d="M0 0H22V78H66V100H0Z"/>
    <path d="M80 0H102V78H136V0H158V100H80Z"/>
    <path d="M172 0H218L246 26V74L218 100H172ZM194 22V78H212L224 66V34L212 22Z" fill-rule="evenodd"/>
    <path d="M260 0H326V22H282V39H310V61H282V78H326V100H260Z"/>
    <path d="M340 0H362L418 100H396Z"/>
    <path d="M396 0H418L362 100H340Z"/>
  </g>
  <rect x="120" y="768" width="360" height="6" fill="${CABINET.signal}"/>
</svg>`;

async function png(svg, size, file) {
  await sharp(Buffer.from(svg)).resize({ width: size, height: size, fit: "contain" }).png({ compressionLevel: 9 }).toFile(file);
}

/** Windows .ico: PNG payloads in an ICONDIR container (Vista+). */
async function ico(svg, sizes, file) {
  const images = await Promise.all(
    sizes.map(async (size) => ({
      size,
      data: await sharp(Buffer.from(svg)).resize({ width: size, height: size, fit: "contain" }).png().toBuffer(),
    })),
  );
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(images.length, 4);
  let offset = 6 + images.length * 16;
  const entries = images.map(({ size, data }) => {
    const entry = Buffer.alloc(16);
    entry.writeUInt8(size >= 256 ? 0 : size, 0);
    entry.writeUInt8(size >= 256 ? 0 : size, 1);
    entry.writeUInt8(0, 2);
    entry.writeUInt8(0, 3);
    entry.writeUInt16LE(1, 4);
    entry.writeUInt16LE(32, 6);
    entry.writeUInt32LE(data.length, 8);
    entry.writeUInt32LE(offset, 12);
    offset += data.length;
    return entry;
  });
  writeFileSync(file, Buffer.concat([header, ...entries, ...images.map((image) => image.data)]));
}

/** macOS .icns: PNG-based entries (ic07…ic14), length-prefixed. */
async function icns(svg, file) {
  const entries = [
    { type: "ic11", size: 32 },
    { type: "ic12", size: 64 },
    { type: "ic07", size: 128 },
    { type: "ic08", size: 256 },
    { type: "ic13", size: 256 },
    { type: "ic09", size: 512 },
    { type: "ic14", size: 512 },
    { type: "ic10", size: 1024 },
  ];
  const datas = await Promise.all(
    entries.map(({ size }) => sharp(Buffer.from(svg)).resize({ width: size, height: size, fit: "contain" }).png().toBuffer()),
  );
  const parts = entries.map(({ type }, index) => {
    const data = datas[index];
    const head = Buffer.alloc(8);
    head.write(type, 0, 4, "ascii");
    head.writeUInt32BE(data.length + 8, 4);
    return Buffer.concat([head, data]);
  });
  const total = 8 + parts.reduce((sum, part) => sum + part.length, 0);
  const header = Buffer.alloc(8);
  header.write("icns", 0, 4, "ascii");
  header.writeUInt32BE(total, 4);
  writeFileSync(file, Buffer.concat([header, ...parts]));
}

mkdirSync(join(WEB, "icons"), { recursive: true });
mkdirSync(join(TAURI, "icons"), { recursive: true });
mkdirSync(join(TAURI, "assets"), { recursive: true });

writeFileSync(join(WEB, "app-icon.svg"), `${markSvg()}\n`);
writeFileSync(join(WEB, "favicon.svg"), `${faviconSvg}\n`);

await png(markSvg(), 192, join(WEB, "icons", "icon-192.png"));
await png(markSvg(), 512, join(WEB, "icons", "icon-512.png"));
await png(markSvg({ maskable: true }), 512, join(WEB, "icons", "maskable-512.png"));
await png(markSvg(), 180, join(WEB, "icons", "apple-touch-icon.png"));

await png(markSvg(), 512, join(TAURI, "icons", "icon.png"));
await png(markSvg(), 32, join(TAURI, "icons", "32x32.png"));
await png(markSvg(), 64, join(TAURI, "icons", "64x64.png"));
await png(markSvg(), 128, join(TAURI, "icons", "128x128.png"));
await png(markSvg(), 256, join(TAURI, "icons", "128x128@2x.png"));
await png(markSvg(), 30, join(TAURI, "icons", "Square30x30Logo.png"));
await png(markSvg(), 44, join(TAURI, "icons", "Square44x44Logo.png"));
await png(markSvg(), 71, join(TAURI, "icons", "Square71x71Logo.png"));
await png(markSvg(), 89, join(TAURI, "icons", "Square89x89Logo.png"));
await png(markSvg(), 107, join(TAURI, "icons", "Square107x107Logo.png"));
await png(markSvg(), 142, join(TAURI, "icons", "Square142x142Logo.png"));
await png(markSvg(), 150, join(TAURI, "icons", "Square150x150Logo.png"));
await png(markSvg(), 284, join(TAURI, "icons", "Square284x284Logo.png"));
await png(markSvg(), 310, join(TAURI, "icons", "Square310x310Logo.png"));
await png(markSvg(), 50, join(TAURI, "icons", "StoreLogo.png"));

await ico(markSvg(), [16, 32, 48, 64, 128, 256], join(TAURI, "icons", "icon.ico"));
await icns(markSvg(), join(TAURI, "icons", "icon.icns"));

await sharp(Buffer.from(posterSvg))
  .png({ compressionLevel: 9 })
  .toFile(join(TAURI, "assets", "ludex-console.png"));

process.stdout.write("cabinet assets written: web icons, Tauri icons (png/ico/icns), Sunshine poster\n");
