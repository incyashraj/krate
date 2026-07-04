import { stdout } from "layer36:io/stdio@0.1.0";
import { current, timezone } from "layer36:locale/info@0.1.0";
import { formatDate } from "layer36:locale/format@0.1.0";
import { nowMillis } from "layer36:time/clock@0.1.0";

const encoder = new TextEncoder();

function writeLine(stream, value) {
  stream.writeAll(encoder.encode(`${value}\n`));
}

export function run() {
  try {
    const out = stdout();
    const locale = current();
    const tz = timezone();
    const date = formatDate(nowMillis(), tz, "medium", locale);

    writeLine(out, "app=layer36-ts-clock");
    writeLine(out, `locale=${locale.bcp47}`);
    writeLine(out, `timezone=${tz}`);
    writeLine(out, `date=${date}`);
    out.flush();
    return 0;
  } catch (_err) {
    return 20;
  }
}
