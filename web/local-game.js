// Read ZIP central directories without loading the full archive into memory.
async function entries(file) {
  const tailOffset = Math.max(0, file.size - 65557);
  const tail = new DataView(await file.slice(tailOffset).arrayBuffer());
  let end = tail.byteLength - 22;
  while (end >= 0 && tail.getUint32(end, true) !== 0x06054b50) end--;
  if (end < 0) throw new Error('This file is not a supported ZIP archive');
  const count = tail.getUint16(end + 10, true);
  const size = tail.getUint32(end + 12, true);
  const offset = tail.getUint32(end + 16, true);
  if (count === 0xffff || offset === 0xffffffff || size === 0xffffffff) throw new Error('ZIP64 archives are not supported');
  const directory = new DataView(await file.slice(offset, offset + size).arrayBuffer());
  const decoder = new TextDecoder();
  const result = [];
  for (let p = 0, i = 0; i < count; i++) {
    if (p + 46 > directory.byteLength || directory.getUint32(p, true) !== 0x02014b50) throw new Error('Invalid ZIP directory');
    const nameLen = directory.getUint16(p + 28, true), extraLen = directory.getUint16(p + 30, true), commentLen = directory.getUint16(p + 32, true);
    const name = decoder.decode(new Uint8Array(directory.buffer, directory.byteOffset + p + 46, nameLen));
    result.push({ name, flags: directory.getUint16(p + 8, true), method: directory.getUint16(p + 10, true), compressed: directory.getUint32(p + 20, true), size: directory.getUint32(p + 24, true), offset: directory.getUint32(p + 42, true) });
    p += 46 + nameLen + extraLen + commentLen;
  }
  return result;
}
async function extract(file, entry, waitUntilVisible) {
  if (entry.flags & 1) throw new Error('Encrypted ZIP entries are not supported');
  const header = new DataView(await file.slice(entry.offset, entry.offset + 30).arrayBuffer());
  if (header.byteLength !== 30 || header.getUint32(0, true) !== 0x04034b50) throw new Error(`Invalid ZIP entry: ${entry.name}`);
  const start = entry.offset + 30 + header.getUint16(26, true) + header.getUint16(28, true);
  const end = start + entry.compressed;
  if (end > file.size) throw new Error('The selected archive is incomplete');
  let position = start;
  let stream = new ReadableStream({
    async pull(controller) {
      if (position >= end) { controller.close(); return; }
      try {
        await waitUntilVisible();
        const next = Math.min(position + 1024 * 1024, end);
        const bytes = await file.slice(position, next).arrayBuffer();
        if (bytes.byteLength !== next - position) throw new Error('The selected file could not be read completely');
        position = next;
        controller.enqueue(new Uint8Array(bytes));
      } catch (error) {
        controller.error(new Error(`The selected file could not be read. Make sure it is fully downloaded and still available: ${error.message || error}`, { cause: error }));
      }
    },
  });
  if (entry.method === 8) {
    try { stream = stream.pipeThrough(new DecompressionStream('deflate-raw')); }
    catch { throw new Error('Your browser cannot unpack ZIP files. Try a current Chrome or Edge browser.'); }
  } else if (entry.method !== 0) throw new Error(`Unsupported ZIP compression for ${entry.name}`);
  const reader = stream.getReader();
  const parts = [];
  let size = 0;
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > entry.size) throw new Error('The archive entry exceeds its expected size');
      parts.push(value);
    }
  } catch (error) {
    throw new Error(`Could not unpack archive entry: ${error.message || error}`, { cause: error });
  } finally { reader.releaseLock(); }
  if (size !== entry.size) throw new Error(`Incomplete ZIP entry: ${entry.name}`);
  return new Blob(parts);
}

export async function importGame(apk, data, base, progress, waitUntilVisible = async () => {}) {
  const readDirectory = async (file, label) => {
    try { return await entries(file); }
    catch (error) { throw new Error(`Could not read the selected ${label}: ${error.message || error}`, { cause: error }); }
  };
  const [apkEntries, dataEntries] = await Promise.all([readDirectory(apk, 'APK'), readDirectory(data, 'data ZIP')]);
  const library = apkEntries.find(e => e.name === 'lib/armeabi-v7a/libsandstorm2.so');
  const files = dataEntries.filter(e => e.name.startsWith('GloftBPHP/') && !e.name.endsWith('/') && !e.name.endsWith('/.nomedia'));
  if (!library || !files.length || !files.some(e => e.name.startsWith('GloftBPHP/data/'))) throw new Error('The selected files do not contain the expected game library and data');
  if (files.some(e => e.name.split('/').some(part => part === '..' || part === '.') || e.name.includes('\\'))) throw new Error('The data archive contains an invalid path');
  const cache = await caches.open('mc2-local-game-v1');
  const url = path => new URL(path, base).href;
  // Remove the completion marker first, so an interrupted import cannot boot.
  await cache.delete(url('manifest.json'));
  await cache.delete(url('converted-intro-v1.mp4'));
  const manifest = {};
  const total = files.reduce((n, e) => n + e.size, library.size);
  let loaded = 0;
  const save = async (entry, file, path) => {
    await waitUntilVisible();
    progress({active: true, path, loaded, total, eta: -1});
    let blob;
    try { blob = await extract(file, entry, waitUntilVisible); }
    catch (error) { throw new Error(`Could not read ${entry.name} from the selected ${file === apk ? 'APK' : 'data ZIP'}: ${error.message || error}`, { cause: error }); }
    try { await cache.put(url(path), new Response(blob)); }
    catch (error) {
      const detail = error.name === 'QuotaExceededError' ? 'Browser storage is full; free space and retry.' : (error.message || String(error));
      throw new Error(`Could not save ${entry.name} in browser storage: ${detail}`, { cause: error });
    }
    loaded += blob.size;
  };
  await save(library, apk, 'libsandstorm2.so');
  for (const entry of files) {
    await save(entry, data, entry.name);
    manifest[entry.name] = entry.size;
  }
  await cache.put(url('manifest.json'), new Response(JSON.stringify(manifest), {headers: {'Content-Type': 'application/json'}}));
  progress({active: false});
}
