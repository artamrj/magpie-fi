import type { SVGProps } from "react";

export type IconName =
  | "alert"
  | "back"
  | "check"
  | "chevron"
  | "edit"
  | "folder"
  | "image"
  | "inbox"
  | "info"
  | "library"
  | "link"
  | "logout"
  | "music"
  | "pause"
  | "play"
  | "plus"
  | "refresh"
  | "search"
  | "send"
  | "shield"
  | "skip"
  | "sparkles"
  | "trash"
  | "user"
  | "users"
  | "x";

const paths: Record<IconName, React.ReactNode> = {
  alert: (
    <>
      <path d="M12 3 2.7 19a1 1 0 0 0 .87 1.5h16.86a1 1 0 0 0 .87-1.5Z" />
      <path d="M12 9v4m0 3h.01" />
    </>
  ),
  back: <path d="m15 5-7 7 7 7" />,
  check: <path d="m5 12 4 4L19 6" />,
  chevron: <path d="m9 18 6-6-6-6" />,
  edit: (
    <>
      <path d="M12 20h9" />
      <path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L8 18l-4 1 1-4Z" />
    </>
  ),
  folder: (
    <path d="M3 6.5A1.5 1.5 0 0 1 4.5 5H9l2 2h8.5A1.5 1.5 0 0 1 21 8.5v9a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5Z" />
  ),
  image: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <circle cx="9" cy="10" r="1.5" />
      <path d="m21 16-5-5-8 8" />
    </>
  ),
  inbox: (
    <>
      <path d="m4 13 2-8h12l2 8v6H4z" />
      <path d="M4 13h5l1 2h4l1-2h5" />
    </>
  ),
  info: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v5m0-8h.01" />
    </>
  ),
  library: (
    <>
      <path d="M9 18V6l11-2v12" />
      <circle cx="6.5" cy="18" r="2.5" />
      <circle cx="17.5" cy="16" r="2.5" />
    </>
  ),
  link: (
    <>
      <path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1" />
      <path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1" />
    </>
  ),
  logout: (
    <>
      <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
      <path d="m16 17 5-5-5-5M21 12H9" />
    </>
  ),
  music: (
    <>
      <path d="M9 18V5l10-2v13" />
      <circle cx="6" cy="18" r="3" />
      <circle cx="16" cy="16" r="3" />
    </>
  ),
  pause: <path d="M9 5v14M15 5v14" />,
  play: <path d="M8 5v14l11-7z" fill="currentColor" stroke="none" />,
  plus: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 8v8M8 12h8" />
    </>
  ),
  refresh: (
    <>
      <path d="M20 7v5h-5" />
      <path d="M19 12a7 7 0 1 0-2 5" />
    </>
  ),
  search: (
    <>
      <circle cx="11" cy="11" r="7" />
      <path d="m20 20-4-4" />
    </>
  ),
  send: (
    <>
      <path d="M21 3 3 10.5l7 2.5 2.5 7.5z" />
      <path d="M21 3 10 13" />
    </>
  ),
  shield: (
    <>
      <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10Z" />
      <path d="m9 12 2 2 4-4" />
    </>
  ),
  skip: (
    <>
      <path d="m5 5 10 7-10 7Z" />
      <path d="M19 5v14" />
    </>
  ),
  sparkles: (
    <>
      <path d="m12 3 1.2 3.8L17 8l-3.8 1.2L12 13l-1.2-3.8L7 8l3.8-1.2Z" />
      <path d="m5 14 .8 2.2L8 17l-2.2.8L5 20l-.8-2.2L2 17l2.2-.8Zm14-2 .8 2.2L22 15l-2.2.8L19 18l-.8-2.2L16 15l2.2-.8Z" />
    </>
  ),
  trash: (
    <>
      <path d="M4 7h16M9 7V4h6v3m3 0-1 14H7L6 7m4 4v6m4-6v6" />
    </>
  ),
  user: (
    <>
      <circle cx="12" cy="8" r="4" />
      <path d="M4 21a8 8 0 0 1 16 0" />
    </>
  ),
  users: (
    <>
      <circle cx="9" cy="8" r="3.5" />
      <path d="M2.5 20a6.5 6.5 0 0 1 13 0" />
      <path d="M16 4.6a3.5 3.5 0 0 1 0 6.8" />
      <path d="M18 14.2a6.5 6.5 0 0 1 3.5 5.8" />
    </>
  ),
  x: <path d="M6 6l12 12M18 6 6 18" />,
};

export function Icon({
  name,
  size = 22,
  ...props
}: SVGProps<SVGSVGElement> & { name: IconName; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      {...props}
    >
      {paths[name]}
    </svg>
  );
}

/** The magpie-fi mark: a magpie on its nest, with the song it took. Same drawing as
 * public/favicon.svg. The wing and eye are cut-outs painted in the tile colour. */
export function BrandMark({ size = 22 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 64 64" fill="currentColor" aria-hidden="true">
      <path d="M12.5 44.5 31 27.5" stroke="currentColor" strokeWidth="5.4" strokeLinecap="round" />
      <path d="M21.5 37.4 30 25c3-3 4-7 6.5-10.1 1.5-1.9 3.5-2.9 5.7-2.7 2.8.1 4.8 1.3 5.7 2.6l5.2 1.1-5.2 1.9c-1.9 1.2-2.5 3.2-2.6 5.3-.3 3.9-1.8 6.4-3.1 8.1-2.2 2.8-4.2 4.6-6.5 5.3z" />
      <path
        d="M28 37.5 33.6 46.3M32 37.3 37.2 46.6"
        stroke="currentColor"
        strokeWidth="1.9"
        strokeLinecap="round"
      />
      <path
        d="M20 43.6C24.5 48.8 30.5 51.5 37.5 51.5 45 51.5 50.8 47.8 54 42.2"
        fill="none"
        stroke="currentColor"
        strokeWidth="4.4"
        strokeLinecap="round"
      />
      <ellipse cx="44.6" cy="40.4" rx="3.2" ry="2.7" transform="rotate(-20 44.6 40.4)" />
      <path d="M46.7 40V30.4c2.4.9 4.6 2 5.9 3.2.5.7-.1 1.7-1 1.4L48.7 34v6z" />
      <path
        fill="var(--brand-cut, var(--button))"
        d="M22.6 34.6c5.2-4.4 11.5-9.1 18.5-11 .4 1.4-.1 2.4-.2 3.5-1.4 3.4-2.9 5.4-4.4 6.5-2.5 1.4-5.5 1.6-8.2 1.5z"
      />
      <circle cx="43.4" cy="16.3" r="1.5" fill="var(--brand-cut, var(--button))" />
    </svg>
  );
}
