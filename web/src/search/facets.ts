import { parseQuery, QueryError, quoted, type FacetKind } from './query';
import type { Facet, Facets } from './types';

/** Case- and width-insensitive form of a label, for matching what the reader typed. */
export const normalized = (value: string): string => value.normalize('NFKC').toLocaleLowerCase();

export type IndexedFacet = {
  facet: Facet;
  /** The readable text a query may use instead of the identifier; undefined when only the identifier will do. */
  alias: string | undefined;
  /** What a typed fragment is matched against. */
  search: string;
};

export type FacetIndex = {
  /** Every value, most common first. */
  entries: IndexedFacet[];
  alias(facet: Facet): string | undefined;
  /** The value an identifier or unique label names; null when the label is ambiguous. */
  lookup(value: string): Facet | null | undefined;
};

/**
 * Facet values by identifier and by readable label. A label shared by two values resolves to
 * nothing rather than silently picking one, and an identifier always wins over a label.
 */
function buildIndex(values: readonly Facet[], kind: string): FacetIndex {
  const identifiers = new Map<string, Facet>();
  const labels = new Map<string, Facet | null>();
  for (const facet of values) {
    if (!identifiers.has(facet.value)) identifiers.set(facet.value, facet);
    const label = normalized(facet.label);
    labels.set(label, labels.has(label) ? null : facet);
  }
  const alias = (facet: Facet): string | undefined => {
    // A source is named by its hostname everywhere it is published, including in a query: that
    // name is already the readable one, and a display name would not survive being shared.
    if (kind === 'source') return undefined;
    if (!facet.label.trim()) return undefined;
    const label = normalized(facet.label);
    if (label === normalized(facet.value)) return facet.value;
    if ((identifiers.get(facet.label) ?? labels.get(label))?.value === facet.value) return facet.label;
    return undefined;
  };
  const entries = values
    .map((facet) => ({ facet, alias: alias(facet), search: normalized(facet.label + ' ' + facet.value) }))
    .sort((a, b) => b.facet.count - a.facet.count || a.facet.label.localeCompare(b.facet.label));
  return {
    entries,
    alias,
    lookup: (value) => identifiers.get(value) ?? labels.get(normalized(value)),
  };
}

// Catalogues are immutable and replaced whole, so an index lives exactly as long as its values.
const indexes = new WeakMap<readonly Facet[], FacetIndex>();

/** The index of a catalogue's values for one facet, built once per catalogue. */
export function facetIndex(values: readonly Facet[], kind: string): FacetIndex {
  let index = indexes.get(values);
  if (!index) indexes.set(values, (index = buildIndex(values, kind)));
  return index;
}

/** The facet a query value names, or a `QueryError` explaining why it names none. */
export function resolveFacet(value: string, values: readonly Facet[], kind: FacetKind): Facet {
  const facet = facetIndex(values, kind).lookup(value);
  if (facet === undefined) throw new QueryError(`Unknown ${kind} “${value}”. Choose a ${kind} from the suggestions.`);
  if (facet === null) throw new QueryError(`Ambiguous ${kind} “${value}”. Choose a specific ${kind} from the suggestions.`);
  return facet;
}

/** Swap internal identifiers in a shared query for their readable labels, where one will do. */
export function readableQuery(raw: string, facets: Facets): string {
  let result = raw;
  try {
    for (const clause of parseQuery(raw).clauses.reverse()) {
      if (clause.kind !== 'facet' || !clause.field) continue;
      const values = facets[clause.field] ?? [];
      const facet = values.find((candidate) => candidate.value === clause.value);
      if (!facet) continue;
      const alias = facetIndex(values, clause.field).alias(facet);
      if (alias === undefined || alias === facet.value) continue;
      result = result.slice(0, clause.start) + `${clause.exclude ? '-' : ''}${clause.field}:${quoted(alias)}` + result.slice(clause.end);
    }
  } catch {
    return raw;
  }
  return result;
}
