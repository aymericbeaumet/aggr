/**
 * Scan raw-text elements without backtracking. Quoted attribute values and comments
 * are consumed once; an unterminated raw-text element consumes the remaining input.
 * Only exact lowercase script/style names are raw text in Svelte; uppercase and
 * dotted names identify components. The match shapes retain the compiler's replacement/source-map contracts.
 * @param {'script' | 'style' | null} tag
 * @param {'lang' | 'preprocess' | 'migrate' | 'tsx'} mode
 */
export function create_tag_scanner(tag, mode) {
	const whitespace = (char) => char !== undefined && /\s/.test(char);
	function language(attributes) {
		let i = 0;
		while (i < attributes.length) {
			while (whitespace(attributes[i]) || attributes[i] === '/') i++;
			const start = i;
			while (i < attributes.length && !whitespace(attributes[i]) && attributes[i] !== '=') i++;
			const name = attributes.slice(start, i);
			while (whitespace(attributes[i])) i++;
			if (attributes[i] !== '=') continue;
			i++;
			while (whitespace(attributes[i])) i++;
			const quote = attributes[i] === '"' || attributes[i] === "'" ? attributes[i++] : null;
			const value_start = i;
			while (i < attributes.length && (quote ? attributes[i] !== quote : !whitespace(attributes[i]))) i++;
			const value = attributes.slice(value_start, i);
			if (quote) i++;
			if (name === 'lang') return value;
		}
		return undefined;
	}
	return {
		lastIndex: 0,
		exec(source) {
			let cursor = this.lastIndex;
			while (cursor < source.length) {
				const start = source.indexOf('<', cursor);
				if (start < 0) break;
				if (source.startsWith('<!--', start)) {
					const end = source.indexOf('-->', start + 4);
					if (end < 0) break;
					cursor = end + 3;
					continue;
				}
				let i = start + 1;
				while (i < source.length && /[a-zA-Z0-9:_-]/.test(source[i])) i++;
				if (i === start + 1) { cursor = i; continue; }
				const name = source.slice(start + 1, i);
				const raw_tag = (name === 'script' || name === 'style') &&
					(whitespace(source[i]) || source[i] === '/' || source[i] === '>');
				// Ordinary Svelte attributes can contain JavaScript expressions, not HTML quotes.
				if (!raw_tag) { cursor = i; continue; }
				const attributes_start = i;
				let quote = null;
				while (i < source.length) {
					const char = source[i];
					if (quote) {
						if (char === quote) quote = null;
					} else if (char === '"' || char === "'") {
						quote = char;
					} else if (char === '>' || char === '<') break;
					i++;
				}
				if (i === source.length) break;
				if (source[i] === '<') { cursor = i; continue; }
				const opening_end = i + 1;
				cursor = opening_end;
				const self_closing = source[i - 1] === '/';
				const attributes = source.slice(attributes_start, self_closing ? i - 1 : i);
				const lang = mode === 'lang' && name === 'script' ? language(attributes) : undefined;
				let closing_start = opening_end;
				let end = opening_end;
				if (!self_closing && lang === undefined) {
					end = -1;
					while ((closing_start = source.indexOf('</', cursor)) !== -1) {
						cursor = closing_start + 2;
						if (!source.startsWith(name, cursor)) continue;
						let close = cursor + name.length;
						// The compiler accepts closing whitespace; preprocess and TSX use exact closes.
						if (mode === 'lang') while (whitespace(source[close])) close++;
						if (source[close] === '>') { end = close + 1; break; }
						cursor = close;
					}
					if (end === -1) break;
				}
				cursor = end;
				if (tag !== null && name !== tag) continue;
				if (mode === 'lang' && lang === undefined) continue;
				if (self_closing && mode !== 'preprocess' && mode !== 'lang') continue;
				const whole = source.slice(start, end);
				const opening = source.slice(start, opening_end);
				const content = self_closing ? undefined : source.slice(opening_end, closing_start);
				const match = mode === 'lang' ? [opening, undefined, lang]
					: mode === 'tsx' ? [whole, undefined, opening, attributes, content]
					: mode === 'migrate' ? [whole, opening, content, source.slice(closing_start, end)]
					: [whole, attributes, content];
				match.index = start;
				match.tag = name;
				this.lastIndex = end;
				return match;
			}
			this.lastIndex = 0;
			return null;
		},
		[Symbol.replace](source, replacement) {
			this.lastIndex = 0;
			const output = [];
			let offset = 0;
			let match;
			while ((match = this.exec(source))) {
				output.push(source.slice(offset, match.index), replacement(...match, match.index, source));
				offset = match.index + match[0].length;
			}
			output.push(source.slice(offset));
			return output.join('');
		}
	};
}
