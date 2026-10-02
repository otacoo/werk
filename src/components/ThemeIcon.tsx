import { Mic, Monitor, Moon, Radiation, Star, Sun } from "lucide-react";
import type { ThemeName } from "../utils/appearance";

export const THEME_OPTIONS: { value: ThemeName; label: string; description: string }[] = [
  { value: "system", label: "System", description: "Follow OS light/dark" },
  { value: "night", label: "Night", description: "Cool neutral dark" },
  { value: "day", label: "Day", description: "Light surfaces" },
  { value: "werk", label: "Werk", description: "Warm sand and olive" },
  { value: "gemma", label: "Gemma", description: "Sky blue and starlight" },
  { value: "migu", label: "Migu", description: "Teal and silver" },
  { value: "nerv", label: "NERV", description: "Black, orange and a green sweep" },
];

export function ThemeIcon({ theme, className }: { theme: ThemeName; className?: string }) {
  if (theme === "system") return <Monitor size={18} className={className} />;
  if (theme === "night") return <Moon size={18} className={className} />;
  if (theme === "day") return <Sun size={18} className={className} />;
  if (theme === "gemma") return <Star size={18} className={className} />;
  if (theme === "migu") return <Mic size={18} className={className} />;
  if (theme === "nerv") return <Radiation size={18} className={className} />;
  return (
    <span className={`text-base font-bold leading-none ${className ?? ""}`}>
      w<span className="text-accent">.</span>
    </span>
  );
}
