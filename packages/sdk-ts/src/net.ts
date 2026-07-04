import { fetch, get, type Request, type Response } from "krate:net/http-client";

const decoder = new TextDecoder();

export { fetch, get };
export type { Request, Response };

export function getText(url: string): string {
  return decoder.decode(get(url));
}
