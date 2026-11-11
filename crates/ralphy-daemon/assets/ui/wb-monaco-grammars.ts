/* ---------------------------------------------------------------------------
   Syntax colors for the two diagram-text languages Monaco does not know:
   DBML (`.dbml`) and Mermaid (`.mmd`, `.mermaid`).

   A Monarch grammar is data: a table of regular expressions per state. It
   holds no logic. Every token name is one the `wb` theme in `wb-monaco.ts`
   already colors, so these languages add no color of their own.
--------------------------------------------------------------------------- */
import type { Monaco } from "./wb-monaco.ts";

/**
 * What a Monarch rule does with its match: a token name, a switch of state, or
 * one action per regexp group.
 */
type MonarchAction = string | { token: string; switchTo?: string } | MonarchAction[];

/**
 * One Monarch rule: a regexp with its action (and the state to push), or the
 * rules of another state.
 */
type MonarchRule = [RegExp, MonarchAction] | [RegExp, MonarchAction, string] | { include: string };

/** A Monarch grammar: the rules of each tokenizer state. */
export type MonarchGrammar = {
  ignoreCase?: boolean;
  tokenizer: { [state: string]: MonarchRule[] };
};

/** What the editor does as the person types in a language. */
export type LanguageConfig = {
  comments: { lineComment?: string; blockComment?: [string, string] };
  brackets: [string, string][];
  autoClosingPairs: { open: string; close: string }[];
};

const BRACKETS: [string, string][] = [
  ["{", "}"],
  ["[", "]"],
  ["(", ")"],
];

const pair = (open: string, close: string) => ({ open, close });

// DBML keeps the table body in its own state: only there does the first word
// of a line name a column and the second word give its type. `Table` pushes
// `tableHead`, the `{` replaces it with `tableBody`, and the `}` pops back.
const DBML: MonarchGrammar = {
  ignoreCase: true,
  tokenizer: {
    root: [[/\b(?:TablePartial|Table)\b/u, "keyword", "@tableHead"], { include: "common" }],
    tableHead: [
      [/\{/u, { token: "delimiter", switchTo: "@tableBody" }],
      [/\}/u, "delimiter", "@pop"],
      { include: "common" },
    ],
    tableBody: [
      [/^(\s*)("[^"]*"|\w+)(\s+)([\w.]+(?:\([^)]*\))?)/u, ["white", "variable", "white", "type"]],
      [/\}/u, "delimiter", "@pop"],
      [/\{/u, "delimiter", "@block"],
      { include: "common" },
    ],
    block: [[/\}/u, "delimiter", "@pop"], [/\{/u, "delimiter", "@block"], { include: "common" }],
    settings: [
      [/\]/u, "delimiter", "@pop"],
      [
        /\b(?:primary\s+key|not\s+null|pk|null|unique|increment|default|note|ref|delete|update)\b/u,
        "attribute.name",
      ],
      { include: "common" },
    ],
    common: [
      [/\s+/u, "white"],
      [/\/\/.*$/u, "comment"],
      [/\/\*/u, "comment", "@comment"],
      [/'''/u, "string", "@longString"],
      [/'(?:[^'\\]|\\.)*'/u, "string"],
      [/'.*$/u, "string"],
      [/`[^`]*`/u, "string"],
      [/`.*$/u, "string"],
      [/\b(?:Project|TableGroup|Ref|Enum|Note|indexes|as)\b/u, "keyword"],
      [/[^\W\d]\w*(?:\.\w+)*/u, ""],
      [/"[^"]*"/u, ""],
      [/\d+(?:\.\d+)?/u, "number"],
      [/<>|[<>-]/u, "operator"],
      [/\[/u, "delimiter", "@settings"],
      [/[{}()\]]/u, "delimiter"],
    ],
    comment: [
      [/\*\//u, "comment", "@pop"],
      [/[^*]+/u, "comment"],
      [/\*/u, "comment"],
    ],
    longString: [
      [/'''/u, "string", "@pop"],
      [/[^']+/u, "string"],
      [/'/u, "string"],
    ],
  },
};

const DBML_CONFIG: LanguageConfig = {
  comments: { lineComment: "//", blockComment: ["/*", "*/"] },
  brackets: BRACKETS,
  autoClosingPairs: [...BRACKETS.map(([o, c]) => pair(o, c)), pair("'", "'"), pair("`", "`")],
};

// The cardinality ends of an ER relation (`||--o{`) come before the `|text|`
// label of a link: both start with a bar.
const MERMAID: MonarchGrammar = {
  tokenizer: {
    root: [
      [/%%\{/u, "metatag", "@directive"],
      [/%%.*$/u, "comment"],
      [/\s+/u, "white"],
      [/"[^"]*"/u, "string"],
      [/".*$/u, "string"],
      [/(?:\|\||\|o|\}o|\}\|)(?:--|\.\.)(?:\|\||o\||o\{|\|\{)/u, "operator"],
      [/\|[^|]*\|/u, "string"],
      [/<?(?:-\.+->|-\.+-|==+>|={3,}|-{3,}|--[>xo)]>?|-[>)]>?)/u, "operator"],
      [
        new RegExp(
          "\\b(?:stateDiagram-v2|stateDiagram|sankey-beta|xychart-beta|block-beta|graph|" +
            "flowchart|sequenceDiagram|classDiagram|erDiagram|journey|gantt|pie|gitGraph|" +
            "mindmap|timeline|quadrantChart|requirementDiagram)\\b",
          "u",
        ),
        "keyword",
      ],
      [
        new RegExp(
          "\\b(?:subgraph|end|direction|participant|actor|[Nn]ote|loop|alt|else|opt|par|" +
            "and|rect|activate|deactivate|classDef|class|style|linkStyle|click|title|section|" +
            "dateFormat|accTitle|accDescr)\\b",
          "u",
        ),
        "keyword",
      ],
      [/\b(?:TB|TD|BT|RL|LR|PK|FK|UK)\b/u, "attribute.name"],
      [/[A-Za-z_]\w*/u, ""],
      [/\d+(?:\.\d+)?/u, "number"],
      [/[[\]{}()]/u, "delimiter"],
    ],
    directive: [
      [/\}%%/u, "metatag", "@pop"],
      [/[^}]+/u, "metatag"],
      [/\}/u, "metatag"],
    ],
  },
};

const MERMAID_CONFIG: LanguageConfig = {
  comments: { lineComment: "%%" },
  brackets: BRACKETS,
  autoClosingPairs: [...BRACKETS.map(([o, c]) => pair(o, c)), pair('"', '"')],
};

/** Teach Monaco the `dbml` and `mermaid` languages: their file names, colors and typing aids. */
export function registerGrammars(monaco: Monaco): void {
  const languages: [string, string[], MonarchGrammar, LanguageConfig][] = [
    ["dbml", [".dbml"], DBML, DBML_CONFIG],
    ["mermaid", [".mmd", ".mermaid"], MERMAID, MERMAID_CONFIG],
  ];
  for (const [id, extensions, grammar, config] of languages) {
    monaco.languages.register({ id, extensions });
    monaco.languages.setMonarchTokensProvider(id, grammar);
    monaco.languages.setLanguageConfiguration(id, config);
  }
}
