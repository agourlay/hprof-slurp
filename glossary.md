# Glossary

## System class
A class that was loaded by the bootstrap loader, or the system class loader. For example, this category includes all classes in the rt.jar file (part of the Java™ runtime environment), such as those in the java.util.* package.

## JNI local
A local variable in native code, for example user-defined JNI code or JVM internal code.

## JNI global
A global variable in native code, for example user-defined JNI code or JVM internal code.

## Thread block
An object that was referenced from an active thread block.

## Thread
A running thread.

## Busy monitor

Everything that called the wait() or notify() methods, or that is synchronized, for example by calling the synchronized(Object) method or by entering a synchronized method. If the method was static, the root is a class, otherwise it is an object.

## Java local
A local variable. For example, input parameters, or locally created objects of methods that are still in the stack of a thread.

## Native stack
Input or output parameters in native code, for example user-defined JNI code or JVM internal code. Many methods have native parts, and the objects that are handled as method parameters become garbage collection roots. For example, parameters used for file, network, I/O, or reflection operations.

## Java stack frame

A Java stack frame, which holds local variables. This type of garbage collection root is only generated if you set the Preferences to treat Java stack frames as objects. For more information, see Java Basics: Threads and thread stack queries.

# Shallow heap

The amount of memory that is consumed by one object. An object requires different amounts of memory depending on the operating system architecture. For example, 32 bits or 64 bits for a reference, 4 bytes for an integer, or 8 bytes for an object of type "Long". Depending on the heap dump format, the size might be adjusted to provide a more realistic consumption of the JVM.

# Retained set

One or more objects plus any objects that are referenced, directly or indirectly, only from those original objects. The retained set is the set of objects that would be removed by garbage collection when an object, or multiple objects, is garbage collected.
The following diagram represents objects in the Java heap. Objects A and B are garbage collection roots, for example method parameters, locally created objects, or objects that are used for wait(), notify(), or synchronized() methods

# Retained heap, or retained size

The total heap size of all the objects in the retained set. This value is the amount of memory that is consumed by all the objects that are kept alive by the objects at the root of the retained set.
In general terms, the shallow heap of an object is the size of the object in the heap. The retained size of the same object is the amount of heap memory that is freed when the object is garbage collected.

### Sources

- https://www.ibm.com/support/knowledgecenter/en/SS3KLZ/com.ibm.java.diagnostics.memory.analyzer.doc/gcroots.html
- https://www.ibm.com/support/knowledgecenter/en/SS3KLZ/com.ibm.java.diagnostics.memory.analyzer.doc/shallowretainedheap.html
