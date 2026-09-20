import { useCallback, useRef, useState, type MutableRefObject } from "react";

export interface DirtyText {
  value: string;
  valueRef: MutableRefObject<string>;
  dirtyRef: MutableRefObject<boolean>;
  setDraft: (next: string) => void;
  markPersisted: (value: string) => void;
  loadPersisted: (value: string) => void;
}

export function useDirtyText(
  initialValue: string,
  initialPersisted = initialValue
): DirtyText {
  const [value, setValue] = useState(initialValue);
  const valueRef = useRef(value);
  const persistedRef = useRef(initialPersisted);
  const dirtyRef = useRef(initialValue !== initialPersisted);

  const setDraft = useCallback((next: string) => {
    valueRef.current = next;
    dirtyRef.current = next !== persistedRef.current;
    setValue(next);
  }, []);

  const markPersisted = useCallback((savedValue: string) => {
    persistedRef.current = savedValue;
    const current = valueRef.current;
    dirtyRef.current = current !== savedValue;
    if (!dirtyRef.current) {
      setValue(savedValue);
    }
  }, []);

  const loadPersisted = useCallback((savedValue: string) => {
    persistedRef.current = savedValue;
    const current = valueRef.current;
    if (dirtyRef.current) {
      dirtyRef.current = current !== savedValue;
      return;
    }
    valueRef.current = savedValue;
    dirtyRef.current = false;
    setValue(savedValue);
  }, []);

  return {
    value,
    valueRef,
    dirtyRef,
    setDraft,
    markPersisted,
    loadPersisted,
  };
}
