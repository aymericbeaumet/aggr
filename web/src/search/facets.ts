import { parseQuery, QueryError } from './query';
import type { Facet, FacetKind, SearchCatalog } from './types';

export const normalized = (value: string) => value.normalize('NFKC').toLowerCase();

interface IndexedFacet { facet: Facet; search: string; alias: string | undefined }

class FacetIndex {
  private identifiers = new Map<string, Facet>();
  private labels = new Map<string, Facet | null>();
  private aliases = new Map<Facet, string | undefined>();
  readonly entries: readonly IndexedFacet[];

  constructor(values: readonly Facet[]) {
    for (const facet of values) {
      if (!this.identifiers.has(facet.value)) this.identifiers.set(facet.value, facet);
      const label = normalized(facet.label);
      this.labels.set(label, this.labels.has(label) ? null : facet);
    }
    this.entries = values.map(facet => {
      const alias = this.computeAlias(facet);
      this.aliases.set(facet, alias);
      return { facet, alias, search: normalized(facet.label + ' ' + facet.value) };
    }).sort((a, b) => b.facet.count - a.facet.count || a.facet.label.localeCompare(b.facet.label));
  }

  lookup(value: string): Facet | null | undefined {
    return this.identifiers.get(value) ?? this.labels.get(normalized(value));
  }

  private computeAlias(facet: Facet): string | undefined {
    if (!facet.label.trim()) return;
    const label = normalized(facet.label);
    if (label === normalized(facet.value)) return facet.value;
    if ((this.identifiers.get(facet.label) ?? this.labels.get(label))?.value === facet.value) return facet.label;
  }

  alias(facet: Facet): string | undefined {
    if (this.aliases.has(facet)) return this.aliases.get(facet);
    const canonical = this.identifiers.get(facet.value);
    return canonical?.label === facet.label ? this.aliases.get(canonical) : this.computeAlias(facet);
  }
}

// Catalogues and contextual count arrays are immutable, replaced with each index/count refresh.
const indexes = new WeakMap<readonly Facet[], FacetIndex>();
export function facetIndex(values: readonly Facet[]): FacetIndex {
  let index = indexes.get(values);
  if (!index) { index = new FacetIndex(values); indexes.set(values, index); }
  return index;
}

export function resolveFacet(value: string, values: readonly Facet[], kind: FacetKind): Facet {
  const facet = facetIndex(values).lookup(value);
  if (facet === undefined) throw new QueryError(`Unknown ${kind} “${value}”. Choose a ${kind} from the suggestions.`);
  if (facet === null) throw new QueryError(`Ambiguous ${kind} “${value}”. Choose a specific ${kind} from the suggestions.`);
  return facet;
}

export function facetAlias(facet: Facet, values: readonly Facet[]): string | undefined {
  return facetIndex(values).alias(facet);
}

export function readableQuery(raw: string, facets: SearchCatalog['facets']): string {
  let result = raw;
  try {
    for (const clause of parseQuery(raw).clauses.reverse()) {
      if (clause.kind !== 'facet' || !clause.field) continue;
      const values = facets[clause.field] || [];
      if (clause.field === 'source') {
        const facet = facetIndex(values).lookup(clause.value);
        if (!facet || facet.value === clause.value) continue;
        const quoted = '"' + facet.value.replace(/\\/g, '\\\\').replace(/"/g, '\\"') + '"';
        result = result.slice(0, clause.start) + `${clause.exclude ? '-' : ''}source:${quoted}` + result.slice(clause.end);
        continue;
      }
      const facet = values.find(facet => facet.value === clause.value);
      if (!facet) continue;
      const alias = facetAlias(facet, values);
      if (alias === undefined || alias === facet.value) continue;
      const quoted = '"' + alias.replace(/\\/g, '\\\\').replace(/"/g, '\\"') + '"';
      result = result.slice(0, clause.start) + `${clause.exclude ? '-' : ''}${clause.field}:${quoted}` + result.slice(clause.end);
    }
  } catch { return raw; }
  return result;
}
