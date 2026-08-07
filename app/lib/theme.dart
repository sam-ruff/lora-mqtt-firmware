import 'package:flutter/material.dart';

/// Same teal as the Walkie-Textie messenger app.
const Color brandPrimary = Color(0xFF128C7E);

ThemeData buildTheme() {
  return ThemeData(
    colorScheme: ColorScheme.fromSeed(seedColor: brandPrimary),
    useMaterial3: true,
    inputDecorationTheme: const InputDecorationTheme(
      border: OutlineInputBorder(),
      isDense: true,
    ),
  );
}
