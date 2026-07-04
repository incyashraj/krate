import { io, locale, time } from "@layer36/sdk";

const loc = locale.current();
const tz = locale.timezone();
const now = time.nowMillis();
const formatted = locale.formatDate(now, tz, "medium", loc);

io.println(`app=layer36-ts-clock`);
io.println(`locale=${loc.bcp47}`);
io.println(`timezone=${tz}`);
io.println(`date=${formatted}`);
