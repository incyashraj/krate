import { io, net } from "@layer36/sdk";

const url = io.args()[0];

if (!url) {
  io.eprintln("usage: layer36-ts-curl <url>");
  throw new Error("missing url");
}

io.print(net.getText(url));
