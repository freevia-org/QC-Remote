import assert from "node:assert/strict";
import test from "node:test";
import { orderUserFolders } from "../src/directory-library-model.ts";

/*
 * The unit lists its user folders in the order it published them, and numbers
 * each row by its position. Synthetic folder names only; the order is the
 * thing under test, not any particular library.
 */

const folder = (name: string, order?: number) => ({ key: `/media/p4/Presets/${name}`, name, isFactory: false, ...(order === undefined ? {} : { order }) });
const names = (folders: readonly { name: string }[]) => folders.map((entry) => entry.name);

test("user folders follow the unit's order instead of the alphabet", () => {
  const ordered = orderUserFolders([folder("Folder A", 2), folder("Folder B", 0), folder("Folder C", 1)]);
  assert.deepEqual(names(ordered), ["Folder B", "Folder C", "Folder A"]);
});

test("My Presets stays first whatever order it carries", () => {
  const ordered = orderUserFolders([folder("Folder A", 0), folder("My Presets", 3), folder("Folder B", 1)]);
  assert.deepEqual(names(ordered), ["My Presets", "Folder A", "Folder B"]);
});

test("the numbered pinned folder is not the default and sorts by its order", () => {
  const ordered = orderUserFolders([folder("My Presets 2", 5), folder("Folder A", 6), folder("My Presets", 4), folder("Folder B", 2)]);
  assert.deepEqual(names(ordered), ["My Presets", "Folder B", "My Presets 2", "Folder A"]);
});

test("without any order the folders keep the arrival order, which is alphabetical", () => {
  const arrived = [folder("Folder A"), folder("Folder B"), folder("Folder C")];
  assert.deepEqual(names(orderUserFolders(arrived)), ["Folder A", "Folder B", "Folder C"]);
  assert.deepEqual(names(orderUserFolders([folder("Folder A"), folder("My Presets"), folder("Folder B")])), ["My Presets", "Folder A", "Folder B"]);
});

test("a folder without an order follows those that have one, in arrival order", () => {
  const ordered = orderUserFolders([folder("Folder A"), folder("Folder B", 1), folder("Folder C"), folder("Folder D", 0)]);
  assert.deepEqual(names(ordered), ["Folder D", "Folder B", "Folder A", "Folder C"]);
});

test("equal orders keep arrival order and the input is not mutated", () => {
  const input = [folder("Folder B", 1), folder("Folder A", 1), folder("Folder C", 0)];
  const before = names(input);
  assert.deepEqual(names(orderUserFolders(input)), ["Folder C", "Folder B", "Folder A"]);
  assert.deepEqual(names(input), before);
});

test("the row badge is the position in the ordered list", () => {
  // coros-directory.tsx numbers each user row index + 1 over this same list.
  const badges = orderUserFolders([folder("Folder A", 1), folder("My Presets", 0), folder("Folder B", 2)]).map((entry, index) => `${index + 1}:${entry.name}`);
  assert.deepEqual(badges, ["1:My Presets", "2:Folder A", "3:Folder B"]);
});
