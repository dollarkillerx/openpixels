/** Minimal store-only ZIP writer. JPEGs do not compress further, so no deflate. */
import { t } from "./i18n";

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) crc = CRC_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

const UTF8_FLAG = 0x0800;

function header(size: number, write: (view: DataView) => void): Uint8Array<ArrayBuffer> {
  const bytes = new Uint8Array(size);
  write(new DataView(bytes.buffer));
  return bytes;
}

/** Without the ZIP64 extension, the format stops at 4 GB and 65535 files. */
const MAX_ZIP_BYTES = 0xffffffff;
const MAX_ZIP_FILES = 0xffff;

/** MS-DOS time and date, as ZIP stores them. */
function dosStamp(date: Date): [number, number] {
  const time = (date.getHours() << 11) | (date.getMinutes() << 5) | Math.floor(date.getSeconds() / 2);
  const day = ((date.getFullYear() - 1980) << 9) | ((date.getMonth() + 1) << 5) | date.getDate();
  return [time, day];
}

export function createZip(files: { name: string; bytes: Uint8Array<ArrayBuffer> }[], now = new Date()): Blob {
  const total = files.reduce((sum, file) => sum + file.bytes.length + 100 + file.name.length * 6, 0);
  if (files.length > MAX_ZIP_FILES || total > MAX_ZIP_BYTES) {
    throw new Error(t("超过 ZIP 的上限（4 GB 或 65535 个文件），请分批导出，或用支持写入文件夹的浏览器"));
  }
  const [time, day] = dosStamp(now);
  const parts: Uint8Array<ArrayBuffer>[] = [];
  const directory: Uint8Array<ArrayBuffer>[] = [];
  let offset = 0;

  for (const { name, bytes } of files) {
    const nameBytes = new TextEncoder().encode(name);
    const crc = crc32(bytes);

    const local = header(30, (view) => {
      view.setUint32(0, 0x04034b50, true);
      view.setUint16(4, 20, true);
      view.setUint16(6, UTF8_FLAG, true);
      view.setUint16(10, time, true);
      view.setUint16(12, day, true);
      view.setUint32(14, crc, true);
      view.setUint32(18, bytes.length, true);
      view.setUint32(22, bytes.length, true);
      view.setUint16(26, nameBytes.length, true);
    });
    parts.push(local, nameBytes, bytes);

    directory.push(
      header(46, (view) => {
        view.setUint32(0, 0x02014b50, true);
        view.setUint16(4, 20, true);
        view.setUint16(6, 20, true);
        view.setUint16(8, UTF8_FLAG, true);
        view.setUint16(12, time, true);
        view.setUint16(14, day, true);
        view.setUint32(16, crc, true);
        view.setUint32(20, bytes.length, true);
        view.setUint32(24, bytes.length, true);
        view.setUint16(28, nameBytes.length, true);
        view.setUint32(42, offset, true);
      }),
      nameBytes,
    );
    offset += local.length + nameBytes.length + bytes.length;
  }

  const directorySize = directory.reduce((sum, part) => sum + part.length, 0);
  const end = header(22, (view) => {
    view.setUint32(0, 0x06054b50, true);
    view.setUint16(8, files.length, true);
    view.setUint16(10, files.length, true);
    view.setUint32(12, directorySize, true);
    view.setUint32(16, offset, true);
  });
  return new Blob([...parts, ...directory, end], { type: "application/zip" });
}
