
export interface AlertBannerProps {
  message: string;
  className?: string;
}

export function AlertBanner({ message, className = "" }: AlertBannerProps) {
  if (!message) return null;

  return (
    <p className={`text-xs text-red-400 bg-transparent ${className}`.trim()}>
      {message}
    </p>
  );
}
