import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../services/hub_service.dart';
import '../src/rust/models.dart';

/// Configuration form, prefilled from the hub, plus a live status card.
class ConfigScreen extends StatefulWidget {
  const ConfigScreen({super.key});

  @override
  State<ConfigScreen> createState() => _ConfigScreenState();
}

class _ConfigScreenState extends State<ConfigScreen> {
  final _formKey = GlobalKey<FormState>();
  late final TextEditingController _ssid;
  late final TextEditingController _password;
  late final TextEditingController _mqttHost;
  late final TextEditingController _mqttPort;
  late final TextEditingController _gwHost;
  late final TextEditingController _gwPort;
  late final TextEditingController _gwFreq;
  late final TextEditingController _gwSf;
  late String _mode;

  @override
  void initState() {
    super.initState();
    final config = context.read<HubService>().snapshot?.config;
    _ssid = TextEditingController(text: config?.wifiSsid ?? '');
    _password = TextEditingController();
    _mqttHost = TextEditingController(text: config?.mqttHost ?? '');
    _mqttPort = TextEditingController(text: '${config?.mqttPort ?? 1883}');
    _gwHost = TextEditingController(text: config?.gwHost ?? '');
    _gwPort = TextEditingController(text: '${config?.gwPort ?? 1700}');
    _gwFreq = TextEditingController(text: '${config?.gwFreqHz ?? 868100000}');
    _gwSf = TextEditingController(text: '${config?.gwSf ?? 7}');
    _mode = config?.mode ?? 'bridge';
  }

  @override
  void dispose() {
    for (final controller in [
      _ssid,
      _password,
      _mqttHost,
      _mqttPort,
      _gwHost,
      _gwPort,
      _gwFreq,
      _gwSf,
    ]) {
      controller.dispose();
    }
    super.dispose();
  }

  Future<void> _save() async {
    if (!(_formKey.currentState?.validate() ?? false)) return;
    final service = context.read<HubService>();
    final update = ConfigUpdate(
      wifiSsid: _ssid.text,
      wifiPassword: _password.text.isEmpty ? null : _password.text,
      mode: _mode,
      mqttHost: _mqttHost.text.isEmpty ? null : _mqttHost.text,
      mqttPort: _mqttHost.text.isEmpty ? null : int.tryParse(_mqttPort.text),
      gwHost: _gwHost.text.isEmpty ? null : _gwHost.text,
      gwPort: _gwHost.text.isEmpty ? null : int.tryParse(_gwPort.text),
      gwFreqHz: _gwHost.text.isEmpty ? null : int.tryParse(_gwFreq.text),
      gwSf: _gwHost.text.isEmpty ? null : int.tryParse(_gwSf.text),
    );
    try {
      final outcome = await service.save(update);
      if (!mounted) return;
      final message = outcome.rebooting
          ? 'Saved. The hub is restarting and will join your WiFi.'
          : 'Saved.';
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(message)));
      service.disconnect();
      Navigator.of(context).pop();
    } catch (e) {
      if (!mounted) return;
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('Not saved: $e')));
    }
  }

  @override
  Widget build(BuildContext context) {
    final service = context.watch<HubService>();
    final status = service.snapshot?.status;
    return Scaffold(
      appBar: AppBar(
        title: const Text('Hub configuration'),
        actions: [
          IconButton(
            icon: const Icon(Icons.refresh),
            onPressed: service.refreshStatus,
            tooltip: 'Refresh status',
          ),
        ],
      ),
      body: Form(
        key: _formKey,
        child: ListView(
          padding: const EdgeInsets.all(16),
          children: [
            if (status != null) _StatusCard(status: status),
            const SizedBox(height: 16),
            Text('WiFi', style: Theme.of(context).textTheme.titleMedium),
            const SizedBox(height: 8),
            TextFormField(
              controller: _ssid,
              decoration: const InputDecoration(labelText: 'Network name (SSID)'),
              validator: (v) =>
                  (v == null || v.isEmpty) ? 'The hub needs a network' : null,
            ),
            const SizedBox(height: 12),
            TextFormField(
              controller: _password,
              obscureText: true,
              decoration: const InputDecoration(
                labelText: 'Password',
                helperText: 'Leave blank to keep the stored password',
              ),
            ),
            const SizedBox(height: 16),
            Text('Mode', style: Theme.of(context).textTheme.titleMedium),
            const SizedBox(height: 8),
            DropdownButtonFormField<String>(
              initialValue: _mode,
              items: const [
                DropdownMenuItem(value: 'bridge', child: Text('MQTT bridge')),
                DropdownMenuItem(
                    value: 'gateway', child: Text('LoRaWAN gateway')),
              ],
              onChanged: (value) => setState(() => _mode = value ?? 'bridge'),
            ),
            const SizedBox(height: 16),
            Text('MQTT broker', style: Theme.of(context).textTheme.titleMedium),
            const SizedBox(height: 8),
            TextFormField(
              controller: _mqttHost,
              decoration: const InputDecoration(labelText: 'Host'),
            ),
            const SizedBox(height: 12),
            TextFormField(
              controller: _mqttPort,
              keyboardType: TextInputType.number,
              decoration: const InputDecoration(labelText: 'Port'),
            ),
            if (_mode == 'gateway') ...[
              const SizedBox(height: 16),
              Text('LoRaWAN network server',
                  style: Theme.of(context).textTheme.titleMedium),
              const SizedBox(height: 8),
              TextFormField(
                controller: _gwHost,
                decoration:
                    const InputDecoration(labelText: 'Host (ChirpStack bridge)'),
              ),
              const SizedBox(height: 12),
              TextFormField(
                controller: _gwPort,
                keyboardType: TextInputType.number,
                decoration: const InputDecoration(labelText: 'Port'),
              ),
              const SizedBox(height: 12),
              TextFormField(
                controller: _gwFreq,
                keyboardType: TextInputType.number,
                decoration: const InputDecoration(labelText: 'Frequency (Hz)'),
              ),
              const SizedBox(height: 12),
              TextFormField(
                controller: _gwSf,
                keyboardType: TextInputType.number,
                decoration:
                    const InputDecoration(labelText: 'Spreading factor (7-12)'),
              ),
            ],
            const SizedBox(height: 24),
            FilledButton(
              onPressed: service.saving ? null : _save,
              child: service.saving
                  ? const SizedBox(
                      height: 20,
                      width: 20,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    )
                  : const Text('Save and restart hub'),
            ),
          ],
        ),
      ),
    );
  }
}

class _StatusCard extends StatelessWidget {
  const _StatusCard({required this.status});

  final HubStatusView status;

  static const _linkStates = ['not set up', 'connecting', 'connected', 'offline'];

  String _link(int state) =>
      state < _linkStates.length ? _linkStates[state] : 'unknown';

  @override
  Widget build(BuildContext context) {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('Live status', style: Theme.of(context).textTheme.titleMedium),
            const SizedBox(height: 8),
            Text('WiFi: ${_link(status.wifiState)}'),
            Text('MQTT: ${_link(status.mqttState)}'),
            Text('Address: ${status.ip}'),
            Text('Messages up ${status.uplink}, down ${status.downlink}, '
                'dropped ${status.dropped}'),
            Text('Up for ${status.uptimeSecs ~/ 60} minutes'),
          ],
        ),
      ),
    );
  }
}
