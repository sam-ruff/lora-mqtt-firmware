import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../services/hub_service.dart';
import 'config_screen.dart';

/// Entry screen: instructions to join the hub's hotspot, then connect.
class ConnectScreen extends StatefulWidget {
  const ConnectScreen({super.key});

  @override
  State<ConnectScreen> createState() => _ConnectScreenState();
}

class _ConnectScreenState extends State<ConnectScreen> {
  final _addressController =
      TextEditingController(text: HubService.defaultHubAddress);

  @override
  void dispose() {
    _addressController.dispose();
    super.dispose();
  }

  Future<void> _connect() async {
    final service = context.read<HubService>();
    final ok = await service.connect(_addressController.text.trim());
    if (!mounted) return;
    if (ok) {
      Navigator.of(context).push(
        MaterialPageRoute(builder: (_) => const ConfigScreen()),
      );
    } else {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text('Could not reach the hub: ${service.lastError}'),
        ),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final service = context.watch<HubService>();
    return Scaffold(
      appBar: AppBar(title: const Text('LoRaMqttHub')),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          const Card(
            child: Padding(
              padding: EdgeInsets.all(16),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    'Set up your hub',
                    style: TextStyle(fontSize: 18, fontWeight: FontWeight.w600),
                  ),
                  SizedBox(height: 12),
                  Text('An unconfigured hub broadcasts its own WiFi hotspot.'),
                  SizedBox(height: 8),
                  Text('Open your phone\'s WiFi settings and join the network '
                      'named LoRaMqttHub followed by the hub\'s id.'),
                  SizedBox(height: 8),
                  Text('Come back here and connect. If the hub is already on '
                      'your network, enter its address instead.'),
                ],
              ),
            ),
          ),
          const SizedBox(height: 16),
          TextField(
            controller: _addressController,
            keyboardType: TextInputType.url,
            decoration: const InputDecoration(labelText: 'Hub address'),
          ),
          const SizedBox(height: 16),
          FilledButton(
            onPressed: service.connecting ? null : _connect,
            child: service.connecting
                ? const SizedBox(
                    height: 20,
                    width: 20,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  )
                : const Text('Connect to hub'),
          ),
        ],
      ),
    );
  }
}
