// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
extern int one();
int registered = 0;
int main() { return one() == 3 && registered == 42 ? 0 : 1; }
