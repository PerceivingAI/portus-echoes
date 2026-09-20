import { describe, expect, it } from "vitest";
import contractCodes from "./user-error-codes.json";
import { errorMessageForCode, USER_ERROR_MESSAGES } from "./errors";
import { isUserErrorCode, USER_ERROR_CODES } from "./types";

describe("user error code contract", () => {
  it("matches the shared Rust/frontend code manifest", () => {
    expect([...USER_ERROR_CODES]).toEqual(contractCodes);
  });

  it("defines exactly one approved message for every code", () => {
    expect(Object.keys(USER_ERROR_MESSAGES)).toEqual([...USER_ERROR_CODES]);
    for (const code of USER_ERROR_CODES) {
      expect(errorMessageForCode(code)).toBe(USER_ERROR_MESSAGES[code]);
      expect(USER_ERROR_MESSAGES[code]).not.toBe("");
    }
  });

  it("maps delivery to the exact approved Settings message", () => {
    expect(USER_ERROR_MESSAGES.delivery).toBe("Delivery error.");
    expect(errorMessageForCode("delivery")).toBe("Delivery error.");
    expect(USER_ERROR_MESSAGES.delivery).not.toBe(USER_ERROR_MESSAGES.unexpected);
  });

  it("validates codes without inspecting message text", () => {
    expect(isUserErrorCode("api_key_rejected")).toBe(true);
    expect(isUserErrorCode("API key rejected")).toBe(false);
    expect(isUserErrorCode({ code: "api_key_rejected" })).toBe(false);
  });

  it("maps malformed or unknown codes directly to unexpected without parsing text", () => {
    const nonCodes = [
      "not_a_real_code",
      "429 Too Many Requests",
      "API key rejected",
      "OpenAI request failed: connect error",
      "local model not found",
      "SendInput failed",
      "clipboard write failed",
      "Delivery error from backend",
    ];

    for (const value of nonCodes) {
      expect(errorMessageForCode(value)).toBe(USER_ERROR_MESSAGES.unexpected);
    }
    expect(errorMessageForCode(null)).toBe(USER_ERROR_MESSAGES.unexpected);
  });
});
