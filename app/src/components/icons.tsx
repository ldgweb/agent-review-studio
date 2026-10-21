import type { CSSProperties, ReactNode } from 'react';

// Inline SVG icons. Most paths come from Lucide (https://lucide.dev), ISC License:
// Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part of Feather (MIT). All other copyright (c) for Lucide are held by Lucide Contributors 2022.
// Permission to use, copy, modify, and/or distribute this software for any purpose with or without fee is hereby granted, provided that the above copyright notice and this permission notice appear in all copies.
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
//
// stroke: currentColor so icons inherit the text color of their control;
// default 14px to sit inside btn-sm and badges without pushing line height.

interface IconProps {
  size?: number;
  style?: CSSProperties;
  className?: string;
}

function make(children: ReactNode) {
  return function Icon({ size = 14, style, className }: IconProps) {
    return (
      <svg
        width={size}
        height={size}
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
        focusable="false"
        className={className}
        style={{ flexShrink: 0, verticalAlign: '-0.125em', ...style }}
      >
        {children}
      </svg>
    );
  };
}

export const IconCheck = make(<path d="M20 6 9 17l-5-5" />);
export const IconX = make(<><path d="M18 6 6 18" /><path d="m6 6 12 12" /></>);
export const IconRotateCcw = make(
  <><path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" /><path d="M3 3v5h5" /></>,
);
export const IconChevronLeft = make(<path d="m15 18-6-6 6-6" />);
export const IconChevronRight = make(<path d="m9 18 6-6-6-6" />);
export const IconArrowLeft = make(<><path d="M19 12H5" /><path d="m12 19-7-7 7-7" /></>);
export const IconFocus = make(
  <>
    <path d="M8 3H5a2 2 0 0 0-2 2v3" />
    <path d="M16 3h3a2 2 0 0 1 2 2v3" />
    <path d="M8 21H5a2 2 0 0 1-2-2v-3" />
    <path d="M16 21h3a2 2 0 0 0 2-2v-3" />
  </>,
);
export const IconGitBranch = make(
  <>
    <path d="M6 3v12" />
    <circle cx="18" cy="6" r="3" />
    <circle cx="6" cy="18" r="3" />
    <path d="M18 9a9 9 0 0 1-9 9" />
  </>,
);
export const IconSparkle = make(
  <path d="M12 3l1.9 5.7a2 2 0 0 0 1.4 1.4L21 12l-5.7 1.9a2 2 0 0 0-1.4 1.4L12 21l-1.9-5.7a2 2 0 0 0-1.4-1.4L3 12l5.7-1.9a2 2 0 0 0 1.4-1.4Z" />,
);
export const IconFlag = make(
  <><path d="M4 15s1-1 4-1 5 2 8 2 4-1 4-1V3s-1 1-4 1-5-2-8-2-4 1-4 1z" /><path d="M4 22v-7" /></>,
);
export const IconDiamond = make(<path d="M12 2l10 10-10 10L2 12z" />);
export const IconComment = make(
  <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />,
);
export const IconAlert = make(
  <>
    <path d="m21.73 18-8-14a2 2 0 0 0-3.46 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z" />
    <path d="M12 9v4" />
    <path d="M12 17h.01" />
  </>,
);
export const IconWrapText = make(
  <>
    <path d="M3 6h18" />
    <path d="M3 12h13a3 3 0 1 1 0 6h-4" />
    <path d="m14 16-2 2 2 2" />
    <path d="M3 18h6" />
  </>,
);
export const IconColumns = make(
  <><rect x="3" y="3" width="18" height="18" rx="2" /><path d="M12 3v18" /></>,
);
export const IconArchive = make(
  <>
    <rect x="2" y="3" width="20" height="5" rx="1" />
    <path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8" />
    <path d="M10 12h4" />
  </>,
);
export const IconRestore = make(
  <>
    <rect x="2" y="3" width="20" height="5" rx="1" />
    <path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8" />
    <path d="m9 15 3-3 3 3" />
    <path d="M12 12v6" />
  </>,
);
export const IconSearch = make(
  <><circle cx="11" cy="11" r="8" /><path d="m21 21-4.3-4.3" /></>,
);
export const IconRefresh = make(
  <>
    <path d="M21 12a9 9 0 1 1-9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" />
    <path d="M21 3v5h-5" />
  </>,
);
