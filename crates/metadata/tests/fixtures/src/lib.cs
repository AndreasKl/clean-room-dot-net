using System;
using System.Collections.Generic;

namespace Fixtures {
    public interface IShape { double Area(); }

    public struct Point {
        public int X;
        public int Y;
        public Point(int x, int y) { X = x; Y = y; }
    }

    public class Circle : IShape {
        public const double Pi = 3.14159;
        private static int s_count;
        public int[,] Grid = new int[2, 3];

        public double Radius { get; set; }
        public event EventHandler Changed;

        public Circle(double r) { Radius = r; s_count++; }

        public double Area() { return Pi * Radius * Radius; }

        public static T First<T>(List<T> items) { return items[0]; }

        public static int FirstInt(List<int> items) { return First(items); } // emits a MethodSpec

        public static int SafeDivide(int a, int b) {
            try { return a / b; }
            catch (DivideByZeroException) { return 0; }
            finally { s_count--; }
        }

        public void Raise() {
            var handler = Changed;
            if (handler != null) handler(this, EventArgs.Empty);
        }

        public class Nested { }
    }

    [Obsolete("fixture")]
    public enum Color { Red, Green, Blue }
}
