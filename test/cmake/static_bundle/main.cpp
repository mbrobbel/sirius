// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
extern int one();
extern int loader_mode();
int registered = 0;
int main() { return one() == 3 && registered == 42 && loader_mode() == 0 ? 0 : 1; }
