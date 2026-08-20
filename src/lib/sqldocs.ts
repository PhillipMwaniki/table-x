/**
 * What a SQL keyword means, and what it does that surprises people.
 *
 * Not a reference manual. Every engine already ships one of those, and copying
 * it here would produce forty entries saying `COUNT` counts things. The entries
 * that earn their place are the ones with a `note`: the behaviour that is
 * correct, documented, and still catches people out — a `LEFT JOIN` quietly
 * turned into an inner one by a `WHERE`, `COUNT(col)` skipping nulls, `IN`
 * against a list containing a null never returning false.
 *
 * Deliberately about forty terms rather than the whole language. A hover that
 * fires on every word teaches the reader to ignore it; one that fires on the
 * hard parts is worth reading.
 *
 * Where the engines genuinely disagree — and they do, on aggregation into a
 * string, on paging, on upsert — the entry carries `byDriver` rather than a
 * generic summary that is subtly wrong everywhere.
 */

export interface SqlDoc {
  /** One line. Shown in bold at the top of the tooltip. */
  summary: string;
  /** The trap. Omitted where the term genuinely has none. */
  note?: string;
  /** Replacements for engines that spell or behave differently. */
  byDriver?: Record<string, { summary?: string; note?: string }>;
}

/**
 * Keys are upper case, and may contain one space.
 *
 * The two-word entries are looked up first, so `LEFT JOIN` wins over `JOIN`
 * when the cursor is on the second word.
 */
export const SQL_DOCS: Record<string, SqlDoc> = {
  // --- joins -------------------------------------------------------------
  "INNER JOIN": {
    summary: "Rows with a match on both sides. The default when you write JOIN alone.",
  },
  "LEFT JOIN": {
    summary: "Every row from the left table, with nulls where the right has no match.",
    note: "A condition on the right table in WHERE turns this back into an inner join, because the null rows it produced fail that test. Put the condition in ON instead.",
  },
  "RIGHT JOIN": {
    summary: "Every row from the right table, with nulls where the left has no match.",
    note: "The same query reads more clearly as a LEFT JOIN with the tables swapped, which is why it is rare.",
  },
  "FULL JOIN": {
    summary: "Every row from both sides, with nulls opposite whichever side is missing.",
    note: "MySQL has no FULL JOIN; it is written as a LEFT JOIN unioned with a RIGHT JOIN.",
  },
  "CROSS JOIN": {
    summary: "Every combination of both sides — the number of rows is the product.",
    note: "A join with no ON clause is a cross join. On two tables of ten thousand rows that is a hundred million.",
  },
  USING: {
    summary: "Join on columns that carry the same name on both sides.",
    note: "The joined column appears once in the result rather than twice, which is a difference from ON that SELECT * makes visible.",
  },

  // --- aggregation -------------------------------------------------------
  COUNT: {
    summary: "How many rows.",
    note: "COUNT(*) counts rows; COUNT(column) counts rows where that column is not null. They differ exactly when the column has nulls, which is when it matters.",
  },
  SUM: {
    summary: "Total of the values, ignoring nulls.",
    note: "SUM over zero rows is NULL, not 0. Wrap it in COALESCE if a number is wanted.",
  },
  AVG: {
    summary: "Mean of the values, ignoring nulls.",
    note: "Nulls are skipped rather than counted as zero, so AVG is not SUM divided by COUNT(*).",
  },
  "GROUP BY": {
    summary: "Collapse rows that share these values into one row each.",
    note: "Every selected column must either appear here or be inside an aggregate. PostgreSQL and SQL Server enforce that; MySQL and SQLite may quietly return an arbitrary row's value.",
  },
  HAVING: {
    summary: "Filter the grouped rows, after aggregation.",
    note: "WHERE runs before grouping and cannot see aggregates; HAVING runs after and can. Filtering that does not involve an aggregate belongs in WHERE, where it removes rows before the work.",
  },
  DISTINCT: {
    summary: "Remove duplicate rows from the result.",
    note: "It applies to the whole row, not to the column it happens to sit before — SELECT DISTINCT a, b dedupes pairs.",
    byDriver: {
      postgres: {
        note: "It applies to the whole row, not the column it sits before. PostgreSQL also has DISTINCT ON (col), which keeps the first row per value and needs ORDER BY to make 'first' mean anything.",
      },
    },
  },
  STRING_AGG: {
    summary: "Join the values of a group into one delimited string.",
    byDriver: {
      mysql: { summary: "MySQL spells this GROUP_CONCAT(expr SEPARATOR ', ')." },
      sqlite: { summary: "SQLite spells this GROUP_CONCAT(expr, ', ')." },
      clickhouse: { summary: "ClickHouse builds an array with groupArray(expr), then joins it." },
    },
  },
  FILTER: {
    summary: "Restrict which rows an aggregate sees, without filtering the query.",
    note: "COUNT(*) FILTER (WHERE paid) counts only the paid rows while other aggregates beside it still see everything. MySQL and SQL Server have no FILTER; the equivalent is COUNT(CASE WHEN paid THEN 1 END).",
  },

  // --- window functions ---------------------------------------------------
  OVER: {
    summary:
      "Make a function a window function: computed across related rows, without collapsing them.",
    note: "This is the difference from GROUP BY — every input row survives and gains a value.",
  },
  "PARTITION BY": {
    summary: "Restart the window calculation for each group of these values.",
    note: "Like GROUP BY, but the rows are kept rather than collapsed.",
  },
  ROW_NUMBER: {
    summary: "1, 2, 3 … within the window, with no ties.",
    note: "Two identical rows still get different numbers, and which one gets 1 is arbitrary unless ORDER BY makes it deterministic.",
  },
  RANK: {
    summary: "Position within the window, where ties share a rank.",
    note: "Ties consume the numbers they skip: two rows at rank 1 mean the next is 3. DENSE_RANK gives 2 instead.",
  },
  DENSE_RANK: {
    summary: "Position within the window, where ties share a rank and nothing is skipped.",
  },
  LAG: {
    summary: "A value from an earlier row of the window.",
    note: "The first row of each partition has no earlier row, so it returns NULL unless a default is given as the third argument.",
  },
  LEAD: {
    summary: "A value from a later row of the window.",
    note: "The last row of each partition returns NULL for the same reason LAG's first row does.",
  },
  NTILE: {
    summary: "Split the window into this many buckets and number them.",
    note: "Rows do not divide evenly in general; the earlier buckets get the extra rows.",
  },
  ROWS: {
    summary: "Define the window frame by counting rows.",
    note: "The default frame with an ORDER BY is RANGE UNBOUNDED PRECEDING AND CURRENT ROW, which lumps tied rows together. ROWS counts physical rows and is usually what a running total wants.",
  },

  // --- set operations ------------------------------------------------------
  UNION: {
    summary: "Rows from both queries, with duplicates removed.",
    note: "Removing duplicates means sorting or hashing everything. UNION ALL skips that and is markedly faster when you already know there are no duplicates.",
  },
  "UNION ALL": {
    summary: "Rows from both queries, keeping duplicates.",
  },
  INTERSECT: { summary: "Rows produced by both queries." },
  EXCEPT: {
    summary: "Rows from the first query that the second does not produce.",
    note: "MySQL before 8.0.31 has neither EXCEPT nor INTERSECT; the equivalent is a LEFT JOIN with an IS NULL test.",
  },

  // --- nulls ----------------------------------------------------------------
  NULL: {
    summary: "Unknown, rather than empty or zero.",
    note: "Nothing equals NULL, including NULL: use IS NULL. Comparisons against it are neither true nor false, which is how rows quietly disappear from a WHERE clause.",
  },
  COALESCE: {
    summary: "The first argument that is not null.",
  },
  NULLIF: {
    summary: "NULL when the two arguments are equal, otherwise the first.",
    note: "Mostly used as NULLIF(x, 0) to turn a division by zero into a null instead of an error.",
  },
  "IS DISTINCT FROM": {
    summary: "Comparison that treats two nulls as equal and one null as different.",
    note: "The null-safe comparison ordinary = cannot do. MySQL spells it <=>; SQL Server has neither and needs an explicit IS NULL test.",
  },

  // --- subqueries -----------------------------------------------------------
  EXISTS: {
    summary: "True when the subquery returns at least one row.",
    note: "Safer than IN against a subquery: if that subquery can return a null, NOT IN never returns true for anything. EXISTS has no such trap.",
  },
  IN: {
    summary: "True when the value matches something in the list or subquery.",
    note: "NOT IN against a list containing a NULL is never true, for any value — the comparison is unknown rather than false. This is the single most common way a correct-looking query returns nothing.",
  },
  ANY: {
    summary: "Compare against every result of a subquery, true if any comparison holds.",
  },

  // --- structure ------------------------------------------------------------
  WITH: {
    summary: "Name a subquery so the main query can read it (a CTE).",
    note: "PostgreSQL before 12 always materialised a CTE, which could be much slower than the same subquery inline. From 12 it inlines when it can, unless you write MATERIALIZED.",
  },
  RECURSIVE: {
    summary: "Let a CTE refer to itself, for trees and generated series.",
    note: "The recursive branch must have a terminating condition; without one the engine runs until it hits a depth limit or fills the disk.",
  },
  CASE: {
    summary: "The first branch whose condition is true, otherwise ELSE.",
    note: "With no ELSE, unmatched rows get NULL rather than being skipped.",
  },
  CAST: {
    summary: "Convert a value to another type.",
    note: "Narrowing a type truncates or errors depending on the engine; casting text that is not a number errors on some and yields NULL on others.",
  },

  // --- paging and ordering --------------------------------------------------
  "ORDER BY": {
    summary: "Sort the result.",
    note: "Without it no order is guaranteed, however stable it looks — a plan change or an index can reorder the rows with no warning.",
  },
  LIMIT: {
    summary: "Return at most this many rows.",
    note: "LIMIT without ORDER BY returns an arbitrary subset, not the first rows in any meaningful sense.",
    byDriver: {
      mssql: {
        summary: "SQL Server spells this TOP n, or OFFSET n ROWS FETCH NEXT m ROWS ONLY.",
      },
    },
  },
  OFFSET: {
    summary: "Skip this many rows before returning any.",
    note: "The skipped rows are still produced and discarded, so paging deep into a large result gets slower the further it goes. Paging by a key value from the previous page does not.",
  },

  // --- writes ---------------------------------------------------------------
  RETURNING: {
    summary: "Give back the rows an INSERT, UPDATE or DELETE affected.",
    note: "Not available on MySQL or SQL Server. SQL Server's nearest equivalent is the OUTPUT clause.",
  },
  "ON CONFLICT": {
    summary: "What to do when an insert collides with a unique constraint (upsert).",
    byDriver: {
      mysql: { summary: "MySQL spells this ON DUPLICATE KEY UPDATE." },
      mssql: { summary: "SQL Server has no upsert clause; the equivalent is MERGE." },
    },
  },
  TRUNCATE: {
    summary: "Empty a table, faster than deleting every row.",
    note: "Usually not transactional and usually does not fire triggers, so it is not simply a quicker DELETE. On some engines it cannot be rolled back at all.",
  },
};

/** How many words a term may have. The lookup tries this many back, at most. */
export const MAX_TERM_WORDS = 3;

/** A doc together with the term it was found under. */
export interface ResolvedDoc extends SqlDoc {
  term: string;
}

/**
 * Find the entry for a word, applying the engine's own wording.
 *
 * `before` is the words preceding the cursor's, nearest last. They are what let
 * `LEFT JOIN` resolve rather than the bare `JOIN` the pointer is actually over,
 * and `IS DISTINCT FROM` rather than `FROM`. Longest match wins, so a specific
 * term always beats the general one it ends with.
 */
export function lookupSqlDoc(
  word: string,
  before: readonly string[],
  driver: string,
): ResolvedDoc | null {
  const words = [...before, word].map((w) => w.toUpperCase());

  // Longest first: three words, then two, then the word alone.
  let term: string | null = null;
  for (let take = Math.min(MAX_TERM_WORDS, words.length); take >= 1; take--) {
    const candidate = words.slice(words.length - take).join(" ");
    if (SQL_DOCS[candidate]) {
      term = candidate;
      break;
    }
  }
  if (term === null) return null;

  const entry = SQL_DOCS[term];
  if (!entry) return null;

  const override = entry.byDriver?.[driver];

  // A driver that replaces the summary drops the generic note with it. The note
  // described the behaviour the summary described; keeping it under a different
  // engine's spelling would be worse than having no note at all.
  const note = override?.note ?? (override?.summary ? undefined : entry.note);

  return {
    term,
    summary: override?.summary ?? entry.summary,
    ...(note === undefined ? {} : { note }),
  };
}
