// Generates platforms/windows/res/orbit.ico from code: an amber rounded square with a ring
// and a small orbiting dot. Run with: node scripts/gen_icon.mjs
import { writeFileSync } from "node:fs";

function coverage(distance) {
  return Math.max(0, Math.min(1, 0.5 - distance));
}

function render(size) {
  const px = new Uint8Array(size * size * 4);
  const s = size / 64;
  const radius = 14 * s;
  const cx = size / 2, cy = size / 2;
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const fx = x + 0.5, fy = y + 0.5;
      // Rounded square.
      const qx = Math.abs(fx - cx) - (size / 2 - radius);
      const qy = Math.abs(fy - cy) - (size / 2 - radius);
      const sd = Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - radius;
      let a = coverage(sd);
      let r = 0xd9, g = 0x77, b = 0x06;
      // Ring.
      const d = Math.hypot(fx - cx, fy - cy);
      const ring = coverage(Math.abs(d - 17 * s) - 4.2 * s);
      r += (255 - r) * ring; g += (255 - g) * ring; b += (255 - b) * ring;
      // Orbiting dot, with a cut in the ring around it.
      const dx = cx + 12.4 * s, dy = cy - 12.4 * s;
      const dd = Math.hypot(fx - dx, fy - dy);
      const cut = coverage(dd - 9.5 * s);
      r += (0xd9 - r) * cut; g += (0x77 - g) * cut; b += (0x06 - b) * cut;
      const dot = coverage(dd - 6 * s);
      r += (255 - r) * dot; g += (255 - g) * dot; b += (255 - b) * dot;
      const i = (y * size + x) * 4;
      // Icon DIBs are straight alpha, BGRA, bottom-up (written below).
      px[i] = b; px[i + 1] = g; px[i + 2] = r; px[i + 3] = Math.round(a * 255);
    }
  }
  return px;
}

const sizes = [16, 20, 24, 32, 40, 48, 64, 256];
const header = Buffer.alloc(6 + sizes.length * 16);
header.writeUInt16LE(1, 2);
header.writeUInt16LE(sizes.length, 4);
let offset = header.length;
const blobs = [];
sizes.forEach((size, n) => {
  const px = render(size);
  const andRow = Math.ceil(size / 8 / 4) * 4;
  const dib = Buffer.alloc(40 + size * size * 4 + andRow * size);
  dib.writeUInt32LE(40, 0);
  dib.writeInt32LE(size, 4);
  dib.writeInt32LE(size * 2, 8);
  dib.writeUInt16LE(1, 12);
  dib.writeUInt16LE(32, 14);
  dib.writeUInt32LE(size * size * 4, 20);
  for (let y = 0; y < size; y++) {
    const src = (size - 1 - y) * size * 4;
    Buffer.from(px.buffer, src, size * 4).copy(dib, 40 + y * size * 4);
  }
  const e = 6 + n * 16;
  header.writeUInt8(size === 256 ? 0 : size, e);
  header.writeUInt8(size === 256 ? 0 : size, e + 1);
  header.writeUInt16LE(1, e + 4);
  header.writeUInt16LE(32, e + 6);
  header.writeUInt32LE(dib.length, e + 8);
  header.writeUInt32LE(offset, e + 12);
  offset += dib.length;
  blobs.push(dib);
});
writeFileSync(new URL("../platforms/windows/res/orbit.ico", import.meta.url), Buffer.concat([header, ...blobs]));
console.log("wrote orbit.ico");
