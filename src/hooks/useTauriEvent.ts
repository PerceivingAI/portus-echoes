import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";

/** Subscribe to a Tauri event for the component's lifetime. */
export function useTauriEvent<T>(event: string, handler: (payload: T) => void) {
  const handlerRef = useRef(handler);
  handlerRef.current = handler;

  useEffect(() => {
    let disposed = false;
    let stopListening: (() => void) | undefined;

    void listen<T>(event, (e) => handlerRef.current(e.payload))
      .then((unlisten) => {
        if (disposed) {
          unlisten();
        } else {
          stopListening = unlisten;
        }
      })
      .catch(() => {
        // Listener registration failure is non-recoverable for this mount, but
        // it must not escape as an unhandled promise rejection.
      });

    return () => {
      disposed = true;
      stopListening?.();
    };
  }, [event]);
}
