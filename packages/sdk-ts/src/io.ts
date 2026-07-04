import { raw } from "krate:io/args";
import { stderr, stdout } from "krate:io/stdio";

const encoder = new TextEncoder();

export function args(): string[] {
  return raw()
    .split("\n")
    .filter((arg) => arg.length > 0);
}

export function print(value: string): void {
  stdout().writeAll(encoder.encode(value));
}

export function println(value: string): void {
  print(`${value}\n`);
}

export function eprint(value: string): void {
  stderr().writeAll(encoder.encode(value));
}

export function eprintln(value: string): void {
  eprint(`${value}\n`);
}
