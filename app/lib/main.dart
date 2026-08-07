import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import 'screens/connect_screen.dart';
import 'services/hub_service.dart';
import 'theme.dart';

/// Dev/mock mode runs the app against the engine's in-memory fake hub so the
/// whole flow is usable and testable without hardware:
///   flutter run --dart-define=MOCK=true
const bool mockMode = bool.fromEnvironment('MOCK', defaultValue: false);

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  runApp(const HubConfigApp());
}

class HubConfigApp extends StatelessWidget {
  const HubConfigApp({super.key});

  @override
  Widget build(BuildContext context) {
    return ChangeNotifierProvider(
      create: (_) => HubService(mockMode: mockMode),
      child: MaterialApp(
        title: 'LoRaMqttHub',
        theme: buildTheme(),
        home: const ConnectScreen(),
      ),
    );
  }
}
