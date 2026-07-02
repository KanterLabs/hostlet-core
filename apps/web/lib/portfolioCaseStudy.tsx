import type { ReactNode } from "react";

const INLINE_TOKEN = /(\*\*[^*\n][^*\n]*\*\*|\[[^\]\n]+\]\((https?:\/\/[^)\s]+)\))/g;

type Block =
  | { kind: "paragraph"; lines: string[] }
  | { kind: "bullets"; items: string[] };

function safeHttpUrl(value: string) {
  try {
    const url = new URL(value);
    return url.protocol === "http:" || url.protocol === "https:" ? url.toString() : null;
  } catch {
    return null;
  }
}

function renderInline(text: string) {
  const nodes: ReactNode[] = [];
  let index = 0;
  for (const match of text.matchAll(INLINE_TOKEN)) {
    const start = match.index ?? 0;
    if (start > index) nodes.push(text.slice(index, start));
    const token = match[0];
    if (token.startsWith("**")) {
      nodes.push(<strong key={`${start}:strong`}>{token.slice(2, -2)}</strong>);
    } else {
      const link = token.match(/^\[([^\]\n]+)\]\((https?:\/\/[^)\s]+)\)$/);
      const href = link ? safeHttpUrl(link[2]) : null;
      nodes.push(
        href ? (
          <a key={`${start}:link`} className="font-medium text-action underline-offset-4 hover:underline" href={href} target="_blank" rel="noreferrer">
            {link?.[1]}
          </a>
        ) : (
          token
        ),
      );
    }
    index = start + token.length;
  }
  if (index < text.length) nodes.push(text.slice(index));
  return nodes;
}

function caseStudyBlocks(value: string) {
  const blocks: Block[] = [];
  let paragraph: string[] = [];
  let bullets: string[] = [];

  function flushParagraph() {
    if (paragraph.length > 0) blocks.push({ kind: "paragraph", lines: paragraph });
    paragraph = [];
  }

  function flushBullets() {
    if (bullets.length > 0) blocks.push({ kind: "bullets", items: bullets });
    bullets = [];
  }

  for (const rawLine of value.replace(/\r\n?/g, "\n").split("\n")) {
    const line = rawLine.trim();
    if (!line) {
      flushParagraph();
      flushBullets();
      continue;
    }
    const bullet = line.match(/^[-*]\s+(.+)$/);
    if (bullet) {
      flushParagraph();
      bullets.push(bullet[1].trim());
      continue;
    }
    flushBullets();
    paragraph.push(line);
  }

  flushParagraph();
  flushBullets();
  return blocks;
}

export function CaseStudyBody({ value }: { value?: string | null }) {
  const clean = value?.trim();
  if (!clean) return null;
  const blocks = caseStudyBlocks(clean);
  return (
    <div className="space-y-3 text-base leading-7 text-ink/80">
      {blocks.map((block, index) => (
        block.kind === "paragraph" ? (
          <p key={index}>{renderInline(block.lines.join(" "))}</p>
        ) : (
          <ul key={index} className="list-disc space-y-1 pl-5">
            {block.items.map((item, itemIndex) => (
              <li key={itemIndex}>{renderInline(item)}</li>
            ))}
          </ul>
        )
      ))}
    </div>
  );
}
