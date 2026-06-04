"use client";

import { useState } from "react";
import { Eye, EyeOff } from "lucide-react";

export function SecretField({
  label,
  value,
  onChange,
  placeholder,
}: {
  label: string;
  value: string;
  placeholder?: string;
  onChange: (value: string) => void;
}) {
  const [revealed, setRevealed] = useState(false);
  return (
    <label className="block">
      <span>{label}</span>
      <span className="mt-1.5 flex rounded-md border border-line bg-surface focus-within:border-action focus-within:ring-2 focus-within:ring-emerald-100">
        <input
          className="border-0 focus-visible:ring-0"
          type={revealed ? "text" : "password"}
          value={value}
          placeholder={placeholder}
          onChange={(event) => onChange(event.target.value)}
        />
        <button
          type="button"
          className="button-secondary min-h-0 rounded-l-none border-0 shadow-none"
          aria-label={revealed ? "Hide secret" : "Reveal secret"}
          onClick={() => setRevealed((next) => !next)}
        >
          {revealed ? <EyeOff size={16} /> : <Eye size={16} />}
        </button>
      </span>
    </label>
  );
}
