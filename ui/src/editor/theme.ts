import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import type { Extension } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { tags as t } from '@lezer/highlight';

/**
 * The editor in the colours of the design: graphite ground, four syntax
 * colours, the accent for the caret. Every colour that depends on the ground
 * is a name from the style sheet, so the light theme changes the editor with
 * everything else. Sizes come from the mockup of the
 * workbench: 13px mono, line height 1.7, a 34px column for author badges.
 */
const frame = EditorView.theme(
  {
    '&': {
      height: '100%',
      color: 'var(--text)',
      backgroundColor: 'transparent',
      fontSize: '13px',
    },
    '&.cm-focused': { outline: 'none' },
    '.cm-scroller': {
      fontFamily: 'var(--mono)',
      lineHeight: '1.7',
      overflowX: 'hidden',
    },
    '.cm-content': { padding: '10px 0', caretColor: 'var(--accent)' },
    '.cm-line': { padding: '0 16px 0 10px' },
    '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--accent)', borderLeftWidth: '2px' },
    '&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection':
      { backgroundColor: 'var(--selection)' },
    '.cm-gutters': {
      backgroundColor: 'transparent',
      color: 'var(--text-3)',
      border: 'none',
      paddingLeft: '8px',
    },
    '.cm-marks': { width: '34px' },
    '.cm-marks .cm-gutterElement': { display: 'flex', alignItems: 'center' },
    '.cm-lineNumbers .cm-gutterElement': { minWidth: '22px', padding: '0', textAlign: 'right' },
    '.cm-activeLine': { backgroundColor: 'var(--veil-1)' },
    '.cm-activeLineGutter': { backgroundColor: 'transparent', color: 'var(--text)' },
    '.cm-line-agent': { backgroundColor: 'var(--agent-tint)' },
    '.cm-line-you': { backgroundColor: 'var(--raised)' },
    '.cm-badge': {
      display: 'inline-block',
      borderRadius: '4px',
      fontSize: '10px',
      lineHeight: '16px',
      textAlign: 'center',
    },
    '.cm-badge-agent': { width: '22px', backgroundColor: 'var(--agent-soft)', color: 'var(--agent-color)' },
    '.cm-badge-you': { width: '30px', backgroundColor: 'var(--text)', color: 'var(--ground)' },
    '.cm-matchingBracket, &.cm-focused .cm-matchingBracket': {
      backgroundColor: 'var(--veil-4)',
      outline: 'none',
    },
    '.cm-selectionMatch': { backgroundColor: 'var(--veil-3)' },
    '.cm-searchMatch': { backgroundColor: 'rgba(245, 181, 68, 0.25)', borderRadius: '2px' },
    '.cm-searchMatch-selected': { backgroundColor: 'rgba(245, 181, 68, 0.5)' },

    // The find bar and other panels.
    '.cm-panels': { backgroundColor: 'var(--ground-deep)', color: 'var(--text)', borderColor: 'var(--line-soft)' },
    '.cm-panels-bottom': { borderTop: '1px solid var(--line-soft)' },
    '.cm-panel.cm-search': { padding: '8px 12px', fontFamily: 'var(--sans)', fontSize: '13px' },
    '.cm-panel.cm-search label': { color: 'var(--text-2)', fontSize: '12px' },
    '.cm-textfield': {
      height: '28px',
      padding: '0 8px',
      border: '1px solid var(--line)',
      borderRadius: '6px',
      backgroundColor: 'var(--ground)',
      color: 'var(--text)',
      fontFamily: 'var(--mono)',
      fontSize: '12.5px',
    },
    '.cm-textfield:focus': { outline: '2px solid var(--accent)', outlineOffset: '-1px' },
    '.cm-button': {
      height: '28px',
      padding: '0 10px',
      border: '1px solid var(--line-strong)',
      borderRadius: '6px',
      backgroundImage: 'none',
      backgroundColor: 'var(--raised)',
      color: 'var(--text)',
      fontFamily: 'var(--sans)',
      fontSize: '13px',
      cursor: 'pointer',
    },
    '.cm-panel.cm-search [name=close]': { color: 'var(--text-2)', fontSize: '18px', cursor: 'pointer' },

    // Completions and other popups.
    '.cm-tooltip': {
      border: '1px solid var(--line-strong)',
      borderRadius: '8px',
      backgroundColor: 'var(--raised)',
      color: 'var(--text)',
      boxShadow: '0 12px 32px rgba(0, 0, 0, 0.45)',
      overflow: 'hidden',
    },
    '.cm-tooltip.cm-tooltip-autocomplete > ul': { fontFamily: 'var(--mono)', fontSize: '12.5px', maxHeight: '260px' },
    '.cm-tooltip.cm-tooltip-autocomplete > ul > li': { padding: '4px 10px', lineHeight: '1.5' },
    '.cm-tooltip-autocomplete ul li[aria-selected]': { backgroundColor: 'var(--pressed)', color: 'var(--text)' },
    '.cm-completionDetail': { color: 'var(--text-3)', fontStyle: 'normal' },
    '.cm-completionMatchedText': { textDecoration: 'none', color: 'var(--accent)' },
    '.cm-placeholder': { color: 'var(--text-3)' },
  },
  { dark: true },
);

const syntax = HighlightStyle.define([
  {
    tag: [t.keyword, t.operatorKeyword, t.modifier, t.controlKeyword, t.definitionKeyword, t.moduleKeyword],
    color: 'var(--syn-keyword)',
  },
  { tag: [t.bool, t.null, t.self, t.atom], color: 'var(--syn-keyword)' },
  { tag: [t.string, t.special(t.string), t.regexp, t.character], color: 'var(--syn-string)' },
  { tag: [t.number, t.unit, t.color], color: 'var(--syn-number)' },
  { tag: [t.comment, t.lineComment, t.blockComment, t.docComment], color: 'var(--syn-comment)', fontStyle: 'italic' },
  { tag: [t.tagName, t.angleBracket], color: 'var(--syn-keyword)' },
  { tag: t.attributeName, color: 'var(--syn-number)' },
  { tag: [t.heading, t.strong], fontWeight: '600' },
  { tag: t.emphasis, fontStyle: 'italic' },
  { tag: [t.link, t.url], color: 'var(--syn-string)', textDecoration: 'underline' },
  { tag: t.strikethrough, textDecoration: 'line-through' },
  { tag: [t.meta, t.processingInstruction], color: 'var(--syn-meta)' },
  { tag: t.invalid, color: 'var(--syn-invalid)' },
]);

export const graphite: Extension = [frame, syntaxHighlighting(syntax)];
