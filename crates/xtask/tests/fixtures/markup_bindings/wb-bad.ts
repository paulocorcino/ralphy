import { component } from "./wb-alpine.ts";

export function wbBad() {
  return component(["toggle"], {
    open: false,
    rows: [{ name: "a" }],
    get count() {
      return this.rows.length;
    },
    async save(row: { name: string }) {
      return row;
    },
  });
}
