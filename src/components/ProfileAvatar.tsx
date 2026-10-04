/// Round profile image with initials fallback (assistant header, messages,
/// and the Persona preview).
export default function ProfileAvatar({
  src,
  name,
  size = 32,
}: {
  src?: string | null;
  name: string;
  size?: number;
}) {
  const cls = "rounded-full border border-border shrink-0";
  if (src) {
    return (
      <img
        src={src}
        alt={name}
        className={`${cls} object-cover`}
        style={{ width: size, height: size }}
      />
    );
  }
  const initials = name.trim().slice(0, 1).toUpperCase() || "?";
  return (
    <div
      className={`${cls} bg-surface-2 flex items-center justify-center text-[0.625rem] text-dim`}
      style={{ width: size, height: size }}
    >
      {initials}
    </div>
  );
}
