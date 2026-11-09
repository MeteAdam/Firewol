// Where rows and section headings sit in the virtual list. A list without groups is one section
// with no heading, which reduces every position to plain multiplication.
import type { FolderGroup } from "./contract";

type Section = { firstLine: number; lines: number; firstRow: number; count: number; bucket: number };

export class ListLayout {
  readonly sections: Section[];
  readonly lines: number;
  /** Height of every line and heading together. */
  readonly height: number;
  readonly grouped: boolean;
  readonly #rows: number;
  readonly #columns: number;
  readonly #line: number;
  readonly #heading: number;

  constructor(rows: number, columns: number, line: number, groups: FolderGroup[], heading: number) {
    this.#rows = rows;
    this.#columns = columns;
    this.#line = line;
    // Groups from before a reload may not match the rows yet; those are ignored until they do.
    const usable = groups.length > 0 && groups.reduce((next, group) => (next === group.start ? next + group.count : -1), 0) === rows;
    this.grouped = usable;
    this.#heading = usable ? heading : 0;
    let firstLine = 0;
    const parts = usable ? groups : [{ start: 0, count: rows, bucket: -1 }];
    this.sections = parts.map((group) => {
      const section = { firstLine, lines: Math.ceil(group.count / columns), firstRow: group.start, count: group.count, bucket: group.bucket };
      firstLine += section.lines;
      return section;
    });
    this.lines = firstLine;
    this.height = firstLine * line + (usable ? parts.length * heading : 0);
  }

  #sectionIndex(line: number) {
    let index = this.sections.length - 1;
    while (index > 0 && this.sections[index].firstLine > line) index--;
    return index;
  }

  /** The line at height `y`; a heading counts as the line below it. */
  lineAt(y: number) {
    for (const [index, section] of this.sections.entries()) {
      const bodyTop = section.firstLine * this.#line + (index + 1) * this.#heading;
      if (y < bodyTop + section.lines * this.#line) return section.firstLine + Math.max(0, Math.floor((y - bodyTop) / this.#line));
    }
    return Math.max(0, this.lines - 1);
  }

  /** The top of a line's own row or tiles. */
  rowTop(line: number) {
    return line * this.#line + (this.#sectionIndex(line) + 1) * this.#heading;
  }

  /** The top of a line together with the heading above it, if it has one. */
  blockTop(line: number) {
    return this.rowTop(line) - (this.headingBefore(line) === undefined ? 0 : this.#heading);
  }

  /** The section bucket whose heading comes right before this line. */
  headingBefore(line: number) {
    if (!this.grouped) return undefined;
    const section = this.sections[this.#sectionIndex(line)];
    return section.firstLine === line ? section.bucket : undefined;
  }

  firstRowOf(line: number) {
    if (line >= this.lines) return this.#rows;
    const section = this.sections[this.#sectionIndex(line)];
    return section.firstRow + (line - section.firstLine) * this.#columns;
  }

  /** The rows of a line: from the first, up to but not including `stop`. */
  rowsOf(line: number): [start: number, stop: number] {
    const section = this.sections[this.#sectionIndex(line)];
    const start = this.firstRowOf(line);
    return [start, Math.min(start + this.#columns, section.firstRow + section.count)];
  }

  lineOfRow(row: number) {
    for (let index = this.sections.length - 1; index >= 0; index--) {
      const section = this.sections[index];
      if (row >= section.firstRow) return section.firstLine + Math.floor((row - section.firstRow) / this.#columns);
    }
    return 0;
  }

  /** The row one line up or down in the same column, or the last one of a shorter line. */
  moveLine(row: number, delta: number) {
    const from = this.lineOfRow(row);
    const target = Math.max(0, Math.min(this.lines - 1, from + delta));
    const [start, stop] = this.rowsOf(target);
    return Math.min(stop - 1, start + (row - this.firstRowOf(from)));
  }
}
