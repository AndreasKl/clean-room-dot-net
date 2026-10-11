using System;

static class Program {
    static string Name() {
        return "Ada";
    }

    static void Greet(string who, string greeting) {
        Console.WriteLine(greeting);
        Console.WriteLine(who);
    }

    static void Main() {
        Greet(Name(), "Hi");
    }
}
