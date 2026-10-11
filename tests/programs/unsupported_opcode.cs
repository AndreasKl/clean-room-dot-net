using System;

static class Program {
    static void Main() {
        Console.WriteLine("before");
        object o = new object();
        Console.WriteLine("unreachable");
    }
}
