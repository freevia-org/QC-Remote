/** In-memory stand-ins for the Durable Object storage and KV the runtime-free
 * modules take, so their state machines run under node:test. */
export type FakeTransaction = {
  get<T>(key: string): Promise<T | undefined>;
  put<T>(key: string, value: T): Promise<void>;
  delete(key: string): Promise<boolean>;
  list<T>(options?: { prefix?: string; limit?: number }): Promise<Map<string, T>>;
  getAlarm(): Promise<number | null>;
  setAlarm(scheduledTime: number): Promise<void>;
  deleteAlarm(): Promise<void>;
};

export function fakeStorage() {
  const data = new Map<string, unknown>();
  let alarm: number | null = null;
  const transaction: FakeTransaction = {
    get: async <T>(key: string) => data.has(key) ? structuredClone(data.get(key)) as T : undefined,
    put: async (key, value) => { data.set(key, structuredClone(value)); },
    delete: async (key) => data.delete(key),
    list: async <T>(options: { prefix?: string } = {}) => new Map([...data].filter(([key]) => key.startsWith(options.prefix ?? "")).sort(([a], [b]) => a < b ? -1 : 1).map(([key, value]) => [key, structuredClone(value)] as [string, T])),
    getAlarm: async () => alarm,
    setAlarm: async (time) => { alarm = time; },
    deleteAlarm: async () => { alarm = null; },
  };
  return {
    data,
    get alarm() { return alarm; },
    transaction: async <T>(callback: (transaction: FakeTransaction) => Promise<T>): Promise<T> => callback(transaction),
  };
}

/** A directory KV with switchable delete failures. */
export function fakeDirectoryKv(initial: Record<string, unknown> = {}) {
  const values = new Map<string, string>(Object.entries(initial).map(([key, value]) => [key, JSON.stringify(value)]));
  const state = { failDelete: false, deletes: [] as string[], puts: [] as string[] };
  return {
    values,
    state,
    get: async (key: string) => values.has(key) ? JSON.parse(values.get(key)!) : null,
    getWithMetadata: async (key: string) => ({ value: values.has(key) ? JSON.parse(values.get(key)!) : null, metadata: null }),
    put: async (key: string, value: string) => { state.puts.push(key); values.set(key, value); },
    delete: async (key: string) => { if (state.failDelete) throw new Error("kv down"); state.deletes.push(key); values.delete(key); },
  };
}

export const equal = async (left: string, right: string) => left === right;
