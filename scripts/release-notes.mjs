import { readFile } from "node:fs/promises";

// The version pull request writes the user-facing notes for its release. The
// in-app update prompt, latest.json and the GitHub release body all come from
// this one reviewed file, so nothing is edited after the build.
export const releaseNotesFile = "release-notes.json";
export const noteLanguages = ["en", "zh-CN"];
const maxItems = 12;
const maxItemLength = 300;

export async function readReleaseNotes(path, version) {
  let notes;
  try {
    notes = JSON.parse(await readFile(path, "utf8"));
  } catch {
    throw new Error(`${releaseNotesFile} is missing or is not valid JSON.`);
  }
  if (notes?.version !== version) {
    throw new Error(`${releaseNotesFile} must describe version ${version}.`);
  }
  return Object.fromEntries(noteLanguages.map((language) => {
    const items = notes[language];
    const valid = Array.isArray(items) && items.length > 0 && items.length <= maxItems && items.every((item) =>
      typeof item === "string" && item.length > 0 && item.length <= maxItemLength && item.trim() === item && !/[\r\n]/.test(item));
    if (!valid) {
      throw new Error(`${releaseNotesFile} needs 1 to ${maxItems} single-line ${language} notes of at most ${maxItemLength} characters.`);
    }
    return [language, items];
  }));
}

export function noteText(items) {
  return items.map((item) => `- ${item}`).join("\n");
}
