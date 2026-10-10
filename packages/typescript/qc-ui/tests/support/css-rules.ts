/** A deliberately small CSS reader: enough to ask "which rules select what, and what do they declare". */
export interface CssRule {
  selector: string;
  declarations: Map<string, string>;
  /** The at-rule preludes this rule sits inside, outermost first, e.g. "@scope (.qc-screen-fixture-root)". */
  context: string[];
}

const stripComments = (css: string) => css.replace(/\/\*[\s\S]*?\*\//g, "");

function parseDeclarations(body: string): Map<string, string> {
  const declarations = new Map<string, string>();
  let depth = 0;
  let start = 0;
  const flush = (end: number) => {
    const text = body.slice(start, end).trim();
    const colon = text.indexOf(":");
    if (colon > 0) declarations.set(text.slice(0, colon).trim().toLowerCase(), text.slice(colon + 1).trim());
  };
  for (let index = 0; index < body.length; index += 1) {
    const char = body[index];
    if (char === "(") depth += 1;
    else if (char === ")") depth -= 1;
    else if (char === ";" && depth === 0) { flush(index); start = index + 1; }
  }
  flush(body.length);
  return declarations;
}

export function parseCss(source: string): CssRule[] {
  const css = stripComments(source);
  const rules: CssRule[] = [];
  const walk = (text: string, context: string[]) => {
    let index = 0;
    while (index < text.length) {
      const open = text.indexOf("{", index);
      if (open < 0) break;
      const prelude = text.slice(index, open).trim().replace(/^[^{}]*;/, (head) => head.slice(head.lastIndexOf(";") + 1)).trim();
      let depth = 1;
      let close = open + 1;
      for (; close < text.length && depth > 0; close += 1) {
        if (text[close] === "{") depth += 1;
        else if (text[close] === "}") depth -= 1;
      }
      const body = text.slice(open + 1, close - 1);
      if (prelude.startsWith("@")) {
        if (!/^@(?:keyframes|font-face|property)/.test(prelude)) walk(body, [...context, prelude]);
      } else {
        rules.push({ selector: prelude, declarations: parseDeclarations(body), context });
      }
      index = close;
    }
  };
  walk(css, []);
  return rules;
}

/** Every class name a selector mentions. */
export const classesOf = (selector: string): string[] => [...selector.matchAll(/\.(-?[_a-zA-Z][\w-]*)/g)].map((match) => match[1]);

/** Every custom property a stylesheet declares or reads. */
export const customPropertiesIn = (source: string): string[] => [...new Set([...stripComments(source).matchAll(/--[\w-]+/g)].map((match) => match[0]))];
