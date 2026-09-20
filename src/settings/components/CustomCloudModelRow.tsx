import React from "react";
import { RadioOptionRow } from "../../components/RadioOptionRow";

export interface CustomCloudModelRowProps {
  isSelected: boolean;
  value: string;
  placeholder?: string;
  onChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
  onBlur: () => void;
  onSelect: () => void;
}

export function CustomCloudModelRow({
  isSelected,
  value,
  placeholder = "Custom Model ID",
  onChange,
  onBlur,
  onSelect,
}: CustomCloudModelRowProps) {
  return (
    <RadioOptionRow
      isSelected={isSelected}
      onClick={(event) => {
        const input = event.currentTarget.querySelector("input");
        if (input && document.activeElement !== input) input.focus();
      }}
      className="cloud-model-option"
    >
      <div className="min-w-0 flex-1 border-b border-hud-border pb-1 transition-colors duration-200 focus-within:border-slate-200">
        <input
          type="text"
          placeholder={placeholder}
          value={value}
          onClick={(event) => event.stopPropagation()}
          onFocus={() => {
            if (!isSelected) {
              onSelect();
            }
          }}
          onChange={onChange}
          onBlur={onBlur}
          className="custom-input w-full truncate bg-transparent py-2 text-sm font-semibold text-slate-200 placeholder:text-sm placeholder:font-semibold outline-none cursor-text focus:border-slate-200"
        />
      </div>
    </RadioOptionRow>
  );
}
