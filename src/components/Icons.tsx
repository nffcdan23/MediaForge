// Icons. SF Symbols can only be used on Apple platforms, so the app uses Lucide's line icons (ISC licence)
// for the symbols the Swift app showed, plus a few drawn here where Lucide has no equivalent.

import type { ReactNode, SVGProps } from "react";

type IconProps = SVGProps<SVGSVGElement> & { size?: number };

function Base({ size = 16, children, ...rest }: IconProps & { children: ReactNode }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" {...rest}>
      {children}
    </svg>
  );
}

/** clock.arrow.circlepath */
export function HistoryIcon(props: IconProps) {
  return (
    <Base {...props}>
      <path d="M3 12a9 9 0 1 0 2.64-6.36L3 8.3" />
      <path d="M3 3.5v4.8h4.8" />
      <path d="M12 7.5V12l3 2" />
    </Base>
  );
}

/** gobackward.10 */
export function Back10(props: IconProps) {
  return (
    <Base {...props} strokeWidth={1.8}>
      <path d="M4.5 12a7.5 7.5 0 1 0 2.2-5.3" />
      <path d="M6.2 2.8v4.1h4.1" />
      <text x="12.2" y="15.4" fill="currentColor" stroke="none" fontSize="7.6" fontWeight="700" textAnchor="middle" fontFamily="system-ui, sans-serif">
        10
      </text>
    </Base>
  );
}

/** goforward.10 */
export function Forward10(props: IconProps) {
  return (
    <Base {...props} strokeWidth={1.8}>
      <path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3" />
      <path d="M17.8 2.8v4.1h-4.1" />
      <text x="11.8" y="15.4" fill="currentColor" stroke="none" fontSize="7.6" fontWeight="700" textAnchor="middle" fontFamily="system-ui, sans-serif">
        10
      </text>
    </Base>
  );
}

/** play.fill */
export function PlayFill({ size = 16, ...rest }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true" {...rest}>
      <path d="M7 4.6v14.8c0 .9 1 1.4 1.7.9l11.2-7.4c.6-.4.6-1.4 0-1.8L8.7 3.7C8 3.2 7 3.7 7 4.6Z" fill="currentColor" />
    </svg>
  );
}

/** pause.fill */
export function PauseFill({ size = 16, ...rest }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true" {...rest}>
      <rect x="5.5" y="4" width="4.6" height="16" rx="1.4" fill="currentColor" />
      <rect x="13.9" y="4" width="4.6" height="16" rx="1.4" fill="currentColor" />
    </svg>
  );
}

/** checkmark.circle.fill, white tick on green, as on finished thumbnails. */
export function CheckBadge({ size = 22 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true" className="done-check">
      <circle cx="12" cy="12" r="10.5" fill="#30d158" />
      <path d="M7.4 12.4l3.1 3.1 6.1-6.6" fill="none" stroke="#fff" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

/** xmark.circle.fill */
export function ClearFill({ size = 15 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <circle cx="12" cy="12" r="10" fill="currentColor" opacity="0.85" />
      <path d="M8.6 8.6l6.8 6.8M15.4 8.6l-6.8 6.8" stroke="var(--window)" strokeWidth="2.2" strokeLinecap="round" />
    </svg>
  );
}

/** The brand gradient, referenced by `color="url(#brand-gradient)"` on icons drawn in it. */
export function BrandGradientDefs() {
  return (
    <svg width="0" height="0" style={{ position: "absolute" }} aria-hidden="true">
      <defs>
        <linearGradient id="brand-gradient" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#ff9938" />
          <stop offset="1" stopColor="#ff5461" />
        </linearGradient>
      </defs>
    </svg>
  );
}

/** hand.raised.fill */
export function RaisedHand({ size = 26, color = "#ff9938" }: { size?: number; color?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <path
        fill={color}
        d="M8.6 3.9c0-.9.7-1.5 1.5-1.5s1.5.7 1.5 1.5v6.2h.6V2.9c0-.9.7-1.5 1.5-1.5s1.5.7 1.5 1.5v7.2h.6V4.3c0-.9.7-1.5 1.5-1.5s1.5.7 1.5 1.5v8.8c0 .4.3.5.5.2l1.1-1.8c.5-.8 1.4-1.1 2.2-.7.7.4.9 1.2.5 2l-2.6 5.3c-1.4 2.9-3.8 4.6-6.9 4.6-3.9 0-6.3-2.7-6.3-6.8V6.1c0-.9.7-1.5 1.5-1.5s1.5.7 1.5 1.5v5.2h.6V3.9Z"
      />
    </svg>
  );
}
