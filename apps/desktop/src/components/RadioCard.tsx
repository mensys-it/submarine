// Radio button drawn as a card, used for the kill switch and split tunneling choices.

/** Props of RadioCard. */
interface Props {
  /** Name of the radio group. */
  name: string;
  checked: boolean;
  disabled?: boolean;
  title: string;
  description: string;
  /** Called when the card is selected. */
  onSelect(): void;
}

/** A card-sized radio button; a native input keeps arrow-key navigation. */
export function RadioCard({ name, checked, disabled, title, description, onSelect }: Props) {
  return (
    <label className="radio-card card" data-checked={checked || undefined} data-disabled={disabled || undefined}>
      <input
        type="radio"
        className="visually-hidden"
        name={name}
        checked={checked}
        disabled={disabled}
        onChange={onSelect}
      />
      <span className="radio-ring" aria-hidden>
        <span className="radio-dot" />
      </span>
      <span className="radio-title">{title}</span>
      <span className="radio-help">{description}</span>
    </label>
  );
}
