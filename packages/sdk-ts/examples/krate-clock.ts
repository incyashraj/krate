import { io, locale, time } from "@krate/sdk";

const loc = locale.current();
const tz = locale.timezone();
const now = time.nowMillis();
const formatted = locale.formatDate(now, tz, "medium", loc);

io.println(`app=krate-ts-clock`);
io.println(`locale=${loc.bcp47}`);
io.println(`timezone=${tz}`);
io.println(`date=${formatted}`);
