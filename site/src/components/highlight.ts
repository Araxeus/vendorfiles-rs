// Turns the home page examples into colored HTML at build time.

const escape = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

/** Colors YAML keys, values and comments. */
export function yaml(src: string): string {
  return src
    .split('\n')
    .map((line) => {
      const e = escape(line);
      if (/^\s*#/.test(line)) return `<span class="c">${e}</span>`;
      const pair = e.match(/^(\s*(?:-\s+)?)([^:\s][^:]*?)(:)(\s.*)?$/);
      if (pair) {
        const [, lead, key, colon, value] = pair;
        return `${lead}<span class="k">${key}</span>${colon}${value ? `<span class="s">${value}</span>` : ''}`;
      }
      const item = e.match(/^(\s*-\s+)(.*)$/);
      if (item) return `${item[1]}<span class="s">${item[2]}</span>`;
      return e;
    })
    .join('\n');
}

const SGR: Record<string, string> = { '2': 'd', '31': 'er', '32': 'ok', '36': 'cy' };

/** Colors a terminal transcript: `$ ` lines are commands, and ANSI color codes become spans. */
export function terminal(src: string): string {
  return src
    .split('\n')
    .map((line) => {
      if (line.startsWith('$ ')) return `<span class="p">$</span> <span class="cmd">${escape(line.slice(2))}</span>`;
      let out = '';
      let open = false;
      for (const part of line.split(/(\x1b\[\d+m)/)) {
        const code = part.match(/^\x1b\[(\d+)m$/)?.[1];
        if (code === undefined) {
          out += escape(part);
          continue;
        }
        if (open) out += '</span>';
        open = code in SGR;
        if (open) out += `<span class="${SGR[code]}">`;
      }
      return open ? `${out}</span>` : out;
    })
    .join('\n');
}
