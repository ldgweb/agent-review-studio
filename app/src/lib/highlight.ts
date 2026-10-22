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

// Per-line syntax highlighting for the diff viewer. refractor (Prism as
// data) tokenizes synchronously and returns a HAST tree we render straight
// to React elements — no innerHTML. Tokenizing line by line mis-colors
// multi-line constructs (block comments, template strings): accepted
// tradeoff, standard in lightweight diff viewers.

for (const grammar of [
  bash, c, cpp, csharp, css, go, java, javascript, json, jsx, markdown,
  markup, php, python, ruby, rust, scss, sql, toml, tsx, typescript, yaml,
]) {
  refractor.register(grammar);
}

/** FileChange.language is the lowercased file extension (core/normalize.rs, language_of) */
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
    const root = refractor.highlight(text, lang) as unknown as { children: Array<HastElement | HastText> };
    return root.children.map((child, i) => toReact(child, i));
  } catch {
    return text;
  }
}
