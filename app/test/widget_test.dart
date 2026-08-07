import 'package:flutter_test/flutter_test.dart';

import 'package:walkie_textie_hub_config/main.dart';

void main() {
  testWidgets('connect screen renders with instructions and address field',
      (tester) async {
    await tester.pumpWidget(const HubConfigApp());

    expect(find.text('Set up your hub'), findsOneWidget);
    expect(find.text('Hub address'), findsOneWidget);
    expect(find.text('Connect to hub'), findsOneWidget);
  });
}
