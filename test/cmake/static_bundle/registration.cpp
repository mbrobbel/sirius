// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
extern int registered;
namespace {
struct Register {
  Register() { registered = 42; }
} registration;
}  // namespace
