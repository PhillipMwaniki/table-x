/**
 * CodeMirror 6 SQL editor.
 *
 * The editor instance is created once and kept across renders; React only feeds
 * it new configuration through compartments. Recreating an EditorView on every
 * render would drop the cursor, selection, and undo history on each keystroke.
 */

import { useEffect, useRef } from "react";
import { EditorState, Compartment } from "@codemirror/state";
import {
  EditorView,
  keymap,
  highlightActiveLine,
  lineNumbers,
  placeholder,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { searchKeymap, highlightSelectionMatches } from "@codemirror/search";
import {
  autocompletion,
  completionKeymap,
  closeBrackets,
  closeBracketsKeymap,
} from "@codemirror/autocomplete";
import type { CompletionContext, CompletionResult } from "@codemirror/autocomplete";
import { sql, PostgreSQL, MySQL, SQLite, StandardSQL } from "@codemirror/lang-sql";
import type { SQLDialect } from "@codemirror/lang-sql";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { sqlHover } from "./sqlHover";
import { tags } from "@lezer/highlight";
import type { CompletionScope } from "@/lib/types";

/** Map a driver id to its CodeMirror dialect, defaulting to standard SQL. */
function dialectFor(driver: string): SQLDialect {
  switch (driver) {
    case "postgres":
      return PostgreSQL;
    case "mysql":
      return MySQL;
    case "sqlite":
      return SQLite;
    default:
      return StandardSQL;
  }
}

/**
 * Syntax colours, drawn from the same tokens as the rest of the app so they
 * follow whichever theme is active.
 *
 * Written against the design tokens rather than fixed hex values, and kept
 * deliberately small: SQL has few lexical categories that matter, and colouring
 * each one differently produces a rainbow that is harder to read than the plain
 * text was. Keywords carry the structure, literals need to be distinguishable
 * from identifiers at a glance, and comments recede.
 */
const highlighting = HighlightStyle.define([
  { tag: tags.keyword, color: "var(--color-accent)", fontWeight: "600" },
  // Operators and punctuation stay in body text: they are structure, but
  // colouring them competes with the keywords for attention.
  { tag: [tags.operator, tags.punctuation, tags.separator], color: "var(--color-text)" },
  {
    tag: [tags.string, tags.special(tags.string)],
    // The same colour the grid tints text values with, so a literal in the
    // editor and a value in the results read as the same kind of thing.
    color: "var(--color-ok)",
  },
  { tag: [tags.number, tags.bool, tags.null], color: "var(--color-warn)" },
  {
    tag: [tags.comment, tags.lineComment, tags.blockComment],
    color: "var(--color-text-muted)",
    fontStyle: "italic",
  },
  {
    tag: [tags.function(tags.variableName), tags.function(tags.propertyName)],
    color: "var(--color-null)",
  },
  // Type names appear in casts and DDL, which is exactly where you are checking
  // them, so they get their own weight rather than their own colour.
  {
    tag: [tags.typeName, tags.standard(tags.typeName)],
    color: "var(--color-text)",
    fontWeight: "600",
  },
  // A quoted identifier is a name, not a string — colouring it as one would
  // suggest "users" and 'users' mean the same thing, which is the single most
  // expensive confusion in SQL.
  { tag: [tags.quote, tags.escape], color: "var(--color-text)" },
  { tag: tags.invalid, color: "var(--color-danger)" },
]);

/** Theme wired to the CSS custom properties, so it follows light/dark. */
const theme = EditorView.theme({
  "&": {
    height: "100%",
    // Follows the data size from settings, like every other surface that shows
    // what is in the database rather than chrome around it.
    fontSize: "var(--text-data)",
    backgroundColor: "var(--color-surface-0)",
    color: "var(--color-text)",
  },
  "&.cm-focused": { outline: "none" },
  ".cm-content": {
    fontFamily: "var(--font-mono)",
    padding: "8px 0",
    caretColor: "var(--color-accent)",
  },
  ".cm-gutters": {
    backgroundColor: "var(--color-surface-0)",
    color: "var(--color-text-muted)",
    border: "none",
    opacity: "0.5",
  },
  ".cm-activeLine": { backgroundColor: "var(--color-surface-1)" },
  ".cm-activeLineGutter": { backgroundColor: "transparent" },
  ".cm-selectionBackground, &.cm-focused .cm-selectionBackground, ::selection": {
    backgroundColor: "color-mix(in oklch, var(--color-accent) 25%, transparent)",
  },
  ".cm-cursor": { borderLeftColor: "var(--color-accent)" },
  ".cm-tooltip": {
    backgroundColor: "var(--color-surface-2)",
    border: "1px solid var(--color-border)",
    borderRadius: "6px",
    fontSize: "12px",
  },
  ".cm-tooltip-autocomplete ul li[aria-selected]": {
    backgroundColor: "var(--color-accent)",
    color: "var(--color-accent-fg)",
  },
  // Marks the token a database error pointed at.
  ".cm-errorRange": {
    textDecoration: "underline wavy var(--color-danger)",
    textUnderlineOffset: "3px",
  },
});

/**
 * Completion sourced from the live schema.
 *
 * Deliberately additive to the dialect's own keyword completion rather than a
 * replacement: users want both `SELECT` and their table names.
 */
function schemaCompletion(scope: CompletionScope | null) {
  return (context: CompletionContext): CompletionResult | null => {
    if (!scope) return null;
    const word = context.matchBefore(/[\w."]*/);
    if (!word || (word.from === word.to && !context.explicit)) return null;

    const options = [
      ...scope.tables.map(([name]) => ({
        label: name,
        type: "class",
        detail: "table",
        boost: 2,
      })),
      // Columns rank highest: once a table is in scope its columns are almost
      // always what the user is reaching for.
      ...scope.tables.flatMap(([table, columns]) =>
        columns.map((column) => ({
          label: column,
          type: "property",
          detail: table,
          boost: 1,
        })),
      ),
      ...scope.schemas.map((name) => ({ label: name, type: "namespace", detail: "schema" })),
      ...scope.functions.map((name) => ({ label: name, type: "function" })),
    ];

    return { from: word.from, options, validFor: /^[\w."]*$/ };
  };
}

export function SqlEditor({
  value,
  onChange,
  onRun,
  driver,
  completion,
  errorPosition,
}: {
  value: string;
  onChange: (sql: string) => void;
  onRun: (selectionOrAll: string) => void;
  driver: string;
  completion: CompletionScope | null;
  /** 1-based offset reported by the database, underlined in the editor. */
  errorPosition?: number | undefined;
}) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const langCompartment = useRef(new Compartment());
  const completionCompartment = useRef(new Compartment());
  // Its own compartment because the wording is per engine: the same keyword
  // gets a different explanation on MySQL than on PostgreSQL.
  const hoverCompartment = useRef(new Compartment());

  // Callbacks live in a ref so the keymap closure always sees the current ones
  // without needing to rebuild the editor when a prop changes identity.
  //
  // Updated in an effect rather than during render: React may render a
  // component and throw the result away, and a ref written during that render
  // would then describe a render that never committed. An effect runs only for
  // the ones that did — and it runs before any keystroke can reach the keymap,
  // which is the only thing that reads this.
  const handlers = useRef({ onChange, onRun });
  useEffect(() => {
    handlers.current = { onChange, onRun };
  }, [onChange, onRun]);

  useEffect(() => {
    if (!host.current || view.current) return;

    const runCommand = (v: EditorView) => {
      // Running the selection when there is one is the behaviour every SQL
      // client has: it is how people test one statement inside a long script.
      const { from, to } = v.state.selection.main;
      const text = from === to ? v.state.doc.toString() : v.state.sliceDoc(from, to);
      handlers.current.onRun(text);
      return true;
    };

    const state = EditorState.create({
      doc: value,
      extensions: [
        lineNumbers(),
        history(),
        closeBrackets(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        autocompletion({ activateOnTyping: true, maxRenderedOptions: 40 }),
        keymap.of([
          // Bound before the defaults so Enter-to-run is not swallowed by the
          // autocomplete or newline handlers.
          { key: "Mod-Enter", run: runCommand, preventDefault: true },
          { key: "Shift-Enter", run: runCommand, preventDefault: true },
          ...closeBracketsKeymap,
          ...completionKeymap,
          ...searchKeymap,
          ...historyKeymap,
          ...defaultKeymap,
          indentWithTab,
        ]),
        langCompartment.current.of(sql({ dialect: dialectFor(driver), upperCaseKeywords: true })),
        hoverCompartment.current.of(sqlHover(driver)),
        syntaxHighlighting(highlighting),
        completionCompartment.current.of([]),
        theme,
        EditorView.lineWrapping,
        placeholder("SELECT * FROM …    (Ctrl+Enter to run)"),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) handlers.current.onChange(update.state.doc.toString());
        }),
      ],
    });

    view.current = new EditorView({ state, parent: host.current });
    return () => {
      view.current?.destroy();
      view.current = null;
    };
    // Intentionally mount-only: subsequent updates go through compartments and
    // transactions below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Push external edits in without clobbering the cursor when the value already
  // matches what the user typed.
  useEffect(() => {
    const v = view.current;
    if (!v) return;
    const current = v.state.doc.toString();
    if (current === value) return;
    v.dispatch({ changes: { from: 0, to: current.length, insert: value } });
  }, [value]);

  useEffect(() => {
    view.current?.dispatch({
      effects: [
        langCompartment.current.reconfigure(
          sql({ dialect: dialectFor(driver), upperCaseKeywords: true }),
        ),
        hoverCompartment.current.reconfigure(sqlHover(driver)),
      ],
    });
  }, [driver]);

  useEffect(() => {
    view.current?.dispatch({
      effects: completionCompartment.current.reconfigure(
        completion ? autocompletion({ override: [schemaCompletion(completion)] }) : [],
      ),
    });
  }, [completion]);

  // Move the cursor to the character the database complained about.
  useEffect(() => {
    const v = view.current;
    if (!v || errorPosition === undefined) return;
    // Postgres reports a 1-based offset; CodeMirror positions are 0-based.
    const pos = Math.min(Math.max(errorPosition - 1, 0), v.state.doc.length);
    v.dispatch({ selection: { anchor: pos }, scrollIntoView: true });
    v.focus();
  }, [errorPosition]);

  return <div ref={host} className="h-full overflow-hidden" />;
}
