import { describe, expect, it } from "vitest";
import { cleanFileName, fileNameFromTags, splitName, tagsFromFileName, tokenize } from "./tagPatterns";

describe("tags from file names", () => {
  it("reads artist and title", () => {
    expect(tagsFromFileName("Daft Punk - One More Time", "%artist% - %title%")).toEqual({ artist: "Daft Punk", title: "One More Time" });
  });

  it("keeps later separators in the last field", () => {
    expect(tagsFromFileName("A - B - C", "%artist% - %title%")).toEqual({ artist: "A", title: "B - C" });
  });

  it("turns track numbers into plain numbers, with totals", () => {
    expect(tagsFromFileName("03 Night Drive", "%track% %title%")).toEqual({ track: "3", title: "Night Drive" });
    expect(tagsFromFileName("03-12 - Song", "%track%-%tracktotal% - %title%")).toEqual({ track: "3", trackTotal: "12", title: "Song" });
    expect(tagsFromFileName("3/12 Song", "%track% %title%")).toEqual({ track: "3", trackTotal: "12", title: "Song" });
  });

  it("drops track text that isn't a number, and dummy parts", () => {
    expect(tagsFromFileName("AB Song", "%track% %title%")).toEqual({ title: "Song" });
    expect(tagsFromFileName("[www.site] Artist - Song", "[%dummy%] %artist% - %title%")).toEqual({ artist: "Artist", title: "Song" });
  });

  it("reports names that don't fit and unknown placeholders", () => {
    expect(tagsFromFileName("NoSeparatorHere", "%artist% - %title%")).toBeNull();
    expect(() => tokenize("%artst% - %title%")).toThrow(/artst/);
    expect(tagsFromFileName("literal only", "literal only")).toBeNull();
  });

  it("matches regular-expression characters literally", () => {
    expect(tagsFromFileName("Artist (Live) [2020] Song", "%artist% (Live) [%year%] %title%")).toEqual({ artist: "Artist", year: "2020", title: "Song" });
  });
});

describe("file names from tags", () => {
  it("pads track numbers so files sort in order", () => {
    expect(fileNameFromTags({ track: "3", title: "Intro" }, "%track% - %title%")).toBe("03 - Intro");
    expect(fileNameFromTags({ track: "7", trackTotal: "120", title: "X" }, "%track% %title%")).toBe("007 X");
  });

  it("replaces characters file systems refuse", () => {
    expect(fileNameFromTags({ artist: "AC/DC", title: 'What? "Now": Live' }, "%artist% - %title%")).toBe("AC-DC - What 'Now' - Live");
  });

  it("trims separators left by empty fields", () => {
    expect(fileNameFromTags({ title: "Only Title" }, "%artist% - %title%")).toBe("Only Title");
    expect(fileNameFromTags({}, "%artist% - %title%")).toBe("");
  });

  it("splits names at the last dot", () => {
    expect(splitName("my.song.mp3")).toEqual({ stem: "my.song", extension: ".mp3" });
    expect(splitName("README")).toEqual({ stem: "README", extension: "" });
    expect(splitName(".hidden")).toEqual({ stem: ".hidden", extension: "" });
    expect(cleanFileName("  a  b  ")).toBe("a b");
  });
});
