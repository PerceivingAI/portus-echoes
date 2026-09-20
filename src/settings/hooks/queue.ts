import type { MutableRefObject } from "react";

export function enqueue(
  queue: MutableRefObject<Promise<void>>,
  operation: () => Promise<void>
): Promise<void> {
  const result = queue.current.then(operation, operation);
  queue.current = result.catch(() => {});
  return result;
}
