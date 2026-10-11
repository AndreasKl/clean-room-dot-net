namespace System {
    public class Object { }

    public abstract class ValueType { }
    public abstract class Enum : ValueType { }

    public struct Void { }
    public struct Boolean { }
    public struct Char { }
    public struct SByte { }
    public struct Byte { }
    public struct Int16 { }
    public struct UInt16 { }
    public struct Int32 { }
    public struct UInt32 { }
    public struct Int64 { }
    public struct UInt64 { }
    public struct Single { }
    public struct Double { }
    public struct Decimal { }
    public struct IntPtr { }
    public struct UIntPtr { }
    public struct RuntimeFieldHandle { }
    public struct RuntimeTypeHandle { }

    public sealed class String { }
    public abstract class Array { }
    public abstract class Type { }
    public abstract class Delegate { }
    public abstract class MulticastDelegate : Delegate { }
    public class Exception { }

    public abstract class Attribute { }
    public sealed class ParamArrayAttribute : Attribute { }

    public interface IDisposable { }

    public static class Console {
        // No body: the runtime implements it as an internal call.
        public static extern void WriteLine(string value);
    }
}
