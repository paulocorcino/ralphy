/* ---------------------------------------------------------------------------
   Syntax colors for the two diagram-text languages Monaco does not know:
   DBML (`.dbml`) and Mermaid (`.mmd`, `.mermaid`).

   A Monarch grammar is data: a table of regular expressions per state. It
   holds no logic. Every token name is one the `wb` theme in `wb-monaco.ts`
   already colors, so these languages add no color of their own.
--------------------------------------------------------------------------- */
import type { Monaco } from "./wb-monaco.ts";

/** What a Monarch rule does with its match: a token name, a switch of state, or one action per regexp group. */
type MonarchAction = string | { token: string; switchTo?: string } | MonarchAction[];

/** One Monarch rule: a regexp with its action (and the state to push), or the rules of another state. */
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
    root: [[/\b(?:TablePartial|Table)\b/, "keyword", "@tableHead"], { include: "common" }],
    tableHead: [
      [/\{/, { token: "delimiter", switchTo: "@tableBody" }],
      [/\}/, "delimiter", "@pop"],
      { include: "common" },
    ],
    tableBody: [
      [/^(\s*)("[^"]*"|\w+)(\s+)([\w.]+(?:\([^)]*\))?)/, ["white", "variable", "white", "type"]],
      [/\}/, "delimiter", "@pop"],
      [/\{/, "delimiter", "@block"],
      { include: "common" },
    ],
    block: [[/\}/, "delimiter", "@pop"], [/\{/, "delimiter", "@block"], { include: "common" }],
    settings: [
      [/\]/, "delimiter", "@pop"],
      [
        /\b(?:primary\s+key|not\s+null|pk|null|unique|increment|default|note|ref|delete|update)\b/,
        "attribute.name",
      ],
      { include: "common" },
    ],
    common: [
      [/\s+/, "white"],
      [/\/\/.*$/, "comment"],
      [/\/\*/, "comment", "@comment"],
      [/'''/, "string", "@longString"],
      [/'(?:[^'\\]|\\.)*'/, "string"],
      [/'.*$/, "string"],
      [/`[^`]*`/, "string"],
      [/`.*$/, "string"],
      [/\b(?:Project|TableGroup|Ref|Enum|Note|indexes|as)\b/, "keyword"],
      [/[^\W\d]\w*(?:\.\w+)*/, ""],
      [/"[^"]*"/, ""],
      [/\d+(?:\.\d+)?/, "number"],
      [/<>|[<>-]/, "operator"],
      [/\[/, "delimiter", "@settings"],
      [/[{}()\]]/, "delimiter"],
    ],
    comment: [
      [/\*\//, "comment", "@pop"],
      [/[^*]+/, "comment"],
      [/\*/, "comment"],
    ],
    longString: [
      [/'''/, "string", "@pop"],
      [/[^']+/, "string"],
      [/'/, "string"],
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
      [/%%\{/, "metatag", "@directive"],
      [/%%.*$/, "comment"],
      [/\s+/, "white"],
      [/"[^"]*"/, "string"],
      [/".*$/, "string"],
      [/(?:\|\||\|o|\}o|\}\|)(?:--|\.\.)(?:\|\||o\||o\{|\|\{)/, "operator"],
      [/\|[^|]*\|/, "string"],
      [/<?(?:-\.+->|-\.+-|==+>|={3,}|-{3,}|--[>xo)]>?|-[>)]>?)/, "operator"],
      [
        /\b(?:stateDiagram-v2|stateDiagram|sankey-beta|xychart-beta|block-beta|graph|flowchart|sequenceDiagram|classDiagram|erDiagram|journey|gantt|pie|gitGraph|mindmap|timeline|quadrantChart|requirementDiagram)\b/,
        "keyword",
      ],
      [
        /\b(?:subgraph|end|direction|participant|actor|[Nn]ote|loop|alt|else|opt|par|and|rect|activate|deactivate|classDef|class|style|linkStyle|click|title|section|dateFormat|accTitle|accDescr)\b/,
        "keyword",
      ],
      [/\b(?:TB|TD|BT|RL|LR|PK|FK|UK)\b/, "attribute.name"],
      [/[A-Za-z_]\w*/, ""],
      [/\d+(?:\.\d+)?/, "number"],
      [/[[\]{}()]/, "delimiter"],
    ],
    directive: [
      [/\}%%/, "metatag", "@pop"],
      [/[^}]+/, "metatag"],
      [/\}/, "metatag"],
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
