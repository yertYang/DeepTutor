import type { ComponentProps } from "react";

type ContentImageProps = Omit<ComponentProps<"img">, "alt"> & { alt: string };

/** User content keeps its natural dimensions and loads with browser credentials. */
export default function ContentImage({
  loading = "lazy",
  decoding = "async",
  alt,
  ...props
}: ContentImageProps) {
  // Arbitrary attachment URLs have no known dimensions; a server optimizer also
  // cannot forward browser cookies. Keep direct loading for these content images.
  // eslint-disable-next-line @next/next/no-img-element
  return <img {...props} alt={alt} loading={loading} decoding={decoding} />;
}
