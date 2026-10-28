import { createElement, type ReactNode } from 'react';
import { refractor } from 'refractor/core';
import bash from 'refractor/bash';
import c from 'refractor/c';
import cpp from 'refractor/cpp';
import csharp from 'refractor/csharp';
import css from 'refractor/css';
import go from 'refractor/go';
import java from 'refractor/java';
import javascript from 'refractor/javascript';
import json from 'refractor/json';
import jsx from 'refractor/jsx';
import markdown from 'refractor/markdown';
import markup from 'refractor/markup';
import php from 'refractor/php';
import python from 'refractor/python';
import ruby from 'refractor/ruby';
import rust from 'refractor/rust';
import scss from 'refractor/scss';
import sql from 'refractor/sql';
import toml from 'refractor/toml';
import tsx from 'refractor/tsx';
import typescript from 'refractor/typescript';
import yaml from 'refractor/yaml';

// Syntax highlighting for the diff viewer. refractor (Prism as data)
// tokenizes synchronously and returns a HAST tree rendered straight to
// React elements, never innerHTML. Whole files are tokenized once (the file as
// it is now and as HEAD has it) and split into lines, so block comments and
// template strings keep their color across lines, as in an editor; a line
// without its file falls back to being tokenized alone.

for (const grammar of [
  bash, c, cpp, csharp, css, go, java, javascript, json, jsx, markdown,
  markup, php, python, ruby, rust, scss, sql, toml, tsx, typescript, yaml,
]) {
  refractor.register(grammar);
}

/** A file's language is its lowercased extension, as the core reports it */
const EXT_TO_LANG: Record<string, string> = {
  ts: 'typescript',
  mts: 'typescript',
  cts: 'typescript',
  tsx: 'tsx',
  js: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  jsx: 'jsx',
  rs: 'rust',
  py: 'python',
  php: 'php',
  go: 'go',
  java: 'java',
  rb: 'ruby',
  c: 'c',
  h: 'c',
  cpp: 'cpp',
  cc: 'cpp',
  cxx: 'cpp',
  hpp: 'cpp',
  cs: 'csharp',
  css: 'css',
  scss: 'scss',
  json: 'json',
  yaml: 'yaml',
  yml: 'yaml',
  md: 'markdown',
  markdown: 'markdown',
  sql: 'sql',
  sh: 'bash',
  bash: 'bash',
  zsh: 'bash',
  html: 'markup',
  xml: 'markup',
  svg: 'markup',
  vue: 'markup',
  toml: 'toml',
};

export function extToLang(language: string): string | null {
  const lang = EXT_TO_LANG[language.toLowerCase()] ?? null;
  return lang && refractor.registered(lang) ? lang : null;
}

interface HastText {
  type: 'text';
  value: string;
}

interface HastElement {
  type: 'element';
  tagName: string;
  properties?: { className?: string[] };
  children: Array<HastElement | HastText>;
}

export type HastNode = HastElement | HastText;

function toReact(node: HastElement | HastText, key: number): ReactNode {
  if (node.type === 'text') return node.value;
  return createElement(
    'span',
    { key, className: (node.properties?.className ?? []).join(' ') },
    node.children.map((child, i) => toReact(child, i)),
  );
}

/** Pathological lines (minified bundles) are not worth tokenizing */
const MAX_HIGHLIGHT_CHARS = 1000;

export function highlightLine(text: string, lang: string | null): ReactNode {
  if (!lang || text.length === 0 || text.length > MAX_HIGHLIGHT_CHARS) return text;
  try {
    const root = refractor.highlight(text, lang) as unknown as { children: HastNode[] };
    return root.children.map((child, i) => toReact(child, i));
  } catch {
    return text;
  }
}

/** Whole files past this size are highlighted line by line instead */
const MAX_FILE_HIGHLIGHT_CHARS = 400_000;

/** Split a HAST forest at newlines; an element cut by one is reopened on
 * the next line, so every line keeps its enclosing token classes. */
function splitLines(nodes: HastNode[]): HastNode[][] {
  const lines: HastNode[][] = [[]];
  for (const node of nodes) {
    const parts: HastNode[][] =
      node.type === 'text'
        ? node.value.split('\n').map((value) => (value ? [{ type: 'text', value }] : []))
        : splitLines(node.children).map((children) => (children.length ? [{ ...node, children }] : []));
    parts.forEach((part, i) => {
      if (i > 0) lines.push([]);
      lines[lines.length - 1].push(...part);
    });
  }
  return lines;
}

/** A whole file's lines, tokenized together; null when not worth it. */
export function highlightFile(lines: string[], lang: string | null): HastNode[][] | null {
  if (!lang) return null;
  const text = lines.join('\n');
  if (text.length > MAX_FILE_HIGHLIGHT_CHARS) return null;
  try {
    const root = refractor.highlight(text, lang) as unknown as { children: HastNode[] };
    const out = splitLines(root.children);
    return out.length === lines.length ? out : null;
  } catch {
    return null;
  }
}

export function renderHast(nodes: HastNode[]): ReactNode {
  return nodes.map((node, i) => toReact(node, i));
}
