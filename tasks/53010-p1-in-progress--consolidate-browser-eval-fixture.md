# Consolidate browser DOM-eval fixture

Run compatible deterministic DOM-eval regression scenarios through one panic-safe serialized browser fixture instead of separate Chrome sessions, while preserving each expression's named semantic assertion and isolation from session/network/profile contracts. Capture launch counts, targeted and warmed-lane CPU/wall measurements, deliberate fault proof, broad validation, and a separate PR.
