// Telegram refuses a widget whose HTML is over 32768 bytes, over 4096 bytes after
// zlib at level 9, or contains a NUL character (Bot API 10.4, sendWidget). The
// bot's widgets live in src/telegram/widgets/; this keeps them inside the limits.
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { deflateSync } from "node:zlib";

const folder = new URL("../../src/telegram/widgets/", import.meta.url);
let failed = false;
for (const name of readdirSync(folder).filter((file) => file.endsWith(".html"))) {
  const html = readFileSync(join(folder.pathname, name));
  const compressed = deflateSync(html, { level: 9 }).length;
  const problems = [];
  if (html.length > 32768) problems.push(`${html.length} bytes, over 32768`);
  if (compressed > 4096) problems.push(`${compressed} bytes compressed, over 4096`);
  if (html.includes(0)) problems.push("contains a NUL character");
  console.log(`${name}: ${html.length} bytes, ${compressed} compressed`);
  if (problems.length) {
    console.error(`${name}: ${problems.join("; ")}`);
    failed = true;
  }
}
process.exit(failed ? 1 : 0);
