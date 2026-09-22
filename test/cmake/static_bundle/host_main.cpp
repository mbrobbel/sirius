// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
extern int loader_mode();
int registered = 0;
int main() { return loader_mode() == 1 && registered == 42 ? 0 : 1; }
