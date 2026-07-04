declare module "krate:io/streams" {
  export interface InputStream {
    read(n: number): Uint8Array;
    readToString(): string;
  }

  export interface OutputStream {
    write(bytes: Uint8Array): number;
    writeAll(bytes: Uint8Array): void;
    flush(): void;
  }
}

declare module "krate:io/stdio" {
  import type { InputStream, OutputStream } from "krate:io/streams";

  export function stdin(): InputStream;
  export function stdout(): OutputStream;
  export function stderr(): OutputStream;
}

declare module "krate:io/args" {
  export function raw(): string;
}

declare module "krate:io/log" {
  export type LogLevel = "trace" | "debug" | "info" | "warn" | "error";

  export interface Field {
    key: string;
    value: string;
  }

  export function emit(level: LogLevel, message: string, fields: Field[]): void;
}

declare module "krate:fs/files" {
  export type OpenMode =
    | { tag: "read" }
    | { tag: "write" }
    | { tag: "read-write" }
    | { tag: "append" };

  export interface FileStat {
    size: bigint;
    modifiedMillis: bigint;
    isDir: boolean;
  }

  export interface File {
    read(n: number): Uint8Array;
    write(bytes: Uint8Array): number;
    seekSet(pos: bigint): bigint;
    seekEnd(): bigint;
    stat(): FileStat;
  }

  export function open(path: string, mode: OpenMode): File;
  export function stat(path: string): FileStat;
  export function list(path: string): string[];
  export function removeFile(path: string): void;
  export function removeDir(path: string): void;
  export function mkdir(path: string): void;
  export function rename(from: string, to: string): void;
}

declare module "krate:net/http-client" {
  export type HttpMethod =
    | "get"
    | "post"
    | "put"
    | "delete"
    | "patch"
    | "head"
    | "options";

  export interface Header {
    name: string;
    value: string;
  }

  export interface Request {
    method: HttpMethod;
    url: string;
    headers: Header[];
    body: Uint8Array;
    timeoutMillis?: number;
  }

  export interface Response {
    status: number;
    headers: Header[];
    body: Uint8Array;
  }

  export function get(url: string): Uint8Array;
  export function fetch(req: Request): Response;
}

declare module "krate:time/clock" {
  export function nowMillis(): bigint;
  export function monotonicNanos(): bigint;
}

declare module "krate:time/sleep" {
  export function sleepMillis(millis: number): void;
}

declare module "krate:locale/info" {
  export interface LocaleId {
    bcp47: string;
  }

  export function current(): LocaleId;
  export function timezone(): string;
}

declare module "krate:locale/format" {
  import type { LocaleId } from "krate:locale/info";

  export type DateStyle = "short" | "medium" | "long" | "full";
  export type NumberStyle = "decimal" | "percent" | "currency";

  export function formatDate(
    millis: bigint,
    tz: string,
    style: DateStyle,
    loc: LocaleId,
  ): string;

  export function formatNumber(
    value: number,
    style: NumberStyle,
    loc: LocaleId,
  ): string;
}
