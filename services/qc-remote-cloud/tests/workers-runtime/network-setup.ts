import { setupNetwork } from "@msw/cloudflare";
import { afterAll, afterEach, beforeAll } from "vitest";

export const network = setupNetwork();

beforeAll(() => network.enable());
afterEach(() => network.resetHandlers());
afterAll(() => network.disable());
