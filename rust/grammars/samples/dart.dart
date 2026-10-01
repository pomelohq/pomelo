import 'dart:math';

/// A point on a plane.
class Point {
  final double x, y;
  const Point(this.x, this.y);

  double distanceTo(Point other) => sqrt(pow(x - other.x, 2) + pow(y - other.y, 2));
}

void main() {
  final a = Point(0, 0), b = Point(3, 4);
  print('distance: ${a.distanceTo(b)}');
}
