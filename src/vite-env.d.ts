/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly LOCAL_MODELS?: string;
  readonly OPENAI_MODELS?: string;
  readonly GROQ_MODELS?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
