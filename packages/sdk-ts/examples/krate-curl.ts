import { io, net } from "@krate/sdk";

const url = io.args()[0];

if (!url) {
  io.eprintln("usage: krate-ts-curl <url>");
  throw new Error("missing url");
}

io.print(net.getText(url));
