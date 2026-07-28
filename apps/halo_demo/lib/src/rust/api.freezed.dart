// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'api.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$HaloApiError {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HaloApiError);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'HaloApiError()';
}


}

/// @nodoc
class $HaloApiErrorCopyWith<$Res>  {
$HaloApiErrorCopyWith(HaloApiError _, $Res Function(HaloApiError) __);
}


/// Adds pattern-matching-related methods to [HaloApiError].
extension HaloApiErrorPatterns on HaloApiError {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( HaloApiError_InvalidArgument value)?  invalidArgument,TResult Function( HaloApiError_SessionNotFound value)?  sessionNotFound,TResult Function( HaloApiError_Core value)?  core,TResult Function( HaloApiError_InternalState value)?  internalState,required TResult orElse(),}){
final _that = this;
switch (_that) {
case HaloApiError_InvalidArgument() when invalidArgument != null:
return invalidArgument(_that);case HaloApiError_SessionNotFound() when sessionNotFound != null:
return sessionNotFound(_that);case HaloApiError_Core() when core != null:
return core(_that);case HaloApiError_InternalState() when internalState != null:
return internalState(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( HaloApiError_InvalidArgument value)  invalidArgument,required TResult Function( HaloApiError_SessionNotFound value)  sessionNotFound,required TResult Function( HaloApiError_Core value)  core,required TResult Function( HaloApiError_InternalState value)  internalState,}){
final _that = this;
switch (_that) {
case HaloApiError_InvalidArgument():
return invalidArgument(_that);case HaloApiError_SessionNotFound():
return sessionNotFound(_that);case HaloApiError_Core():
return core(_that);case HaloApiError_InternalState():
return internalState(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( HaloApiError_InvalidArgument value)?  invalidArgument,TResult? Function( HaloApiError_SessionNotFound value)?  sessionNotFound,TResult? Function( HaloApiError_Core value)?  core,TResult? Function( HaloApiError_InternalState value)?  internalState,}){
final _that = this;
switch (_that) {
case HaloApiError_InvalidArgument() when invalidArgument != null:
return invalidArgument(_that);case HaloApiError_SessionNotFound() when sessionNotFound != null:
return sessionNotFound(_that);case HaloApiError_Core() when core != null:
return core(_that);case HaloApiError_InternalState() when internalState != null:
return internalState(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String message)?  invalidArgument,TResult Function()?  sessionNotFound,TResult Function( String message)?  core,TResult Function()?  internalState,required TResult orElse(),}) {final _that = this;
switch (_that) {
case HaloApiError_InvalidArgument() when invalidArgument != null:
return invalidArgument(_that.message);case HaloApiError_SessionNotFound() when sessionNotFound != null:
return sessionNotFound();case HaloApiError_Core() when core != null:
return core(_that.message);case HaloApiError_InternalState() when internalState != null:
return internalState();case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String message)  invalidArgument,required TResult Function()  sessionNotFound,required TResult Function( String message)  core,required TResult Function()  internalState,}) {final _that = this;
switch (_that) {
case HaloApiError_InvalidArgument():
return invalidArgument(_that.message);case HaloApiError_SessionNotFound():
return sessionNotFound();case HaloApiError_Core():
return core(_that.message);case HaloApiError_InternalState():
return internalState();}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String message)?  invalidArgument,TResult? Function()?  sessionNotFound,TResult? Function( String message)?  core,TResult? Function()?  internalState,}) {final _that = this;
switch (_that) {
case HaloApiError_InvalidArgument() when invalidArgument != null:
return invalidArgument(_that.message);case HaloApiError_SessionNotFound() when sessionNotFound != null:
return sessionNotFound();case HaloApiError_Core() when core != null:
return core(_that.message);case HaloApiError_InternalState() when internalState != null:
return internalState();case _:
  return null;

}
}

}

/// @nodoc


class HaloApiError_InvalidArgument extends HaloApiError {
  const HaloApiError_InvalidArgument({required this.message}): super._();


 final  String message;

/// Create a copy of HaloApiError
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HaloApiError_InvalidArgumentCopyWith<HaloApiError_InvalidArgument> get copyWith => _$HaloApiError_InvalidArgumentCopyWithImpl<HaloApiError_InvalidArgument>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HaloApiError_InvalidArgument&&(identical(other.message, message) || other.message == message));
}


@override
int get hashCode => Object.hash(runtimeType,message);

@override
String toString() {
  return 'HaloApiError.invalidArgument(message: $message)';
}


}

/// @nodoc
abstract mixin class $HaloApiError_InvalidArgumentCopyWith<$Res> implements $HaloApiErrorCopyWith<$Res> {
  factory $HaloApiError_InvalidArgumentCopyWith(HaloApiError_InvalidArgument value, $Res Function(HaloApiError_InvalidArgument) _then) = _$HaloApiError_InvalidArgumentCopyWithImpl;
@useResult
$Res call({
 String message
});




}
/// @nodoc
class _$HaloApiError_InvalidArgumentCopyWithImpl<$Res>
    implements $HaloApiError_InvalidArgumentCopyWith<$Res> {
  _$HaloApiError_InvalidArgumentCopyWithImpl(this._self, this._then);

  final HaloApiError_InvalidArgument _self;
  final $Res Function(HaloApiError_InvalidArgument) _then;

/// Create a copy of HaloApiError
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? message = null,}) {
  return _then(HaloApiError_InvalidArgument(
message: null == message ? _self.message : message // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class HaloApiError_SessionNotFound extends HaloApiError {
  const HaloApiError_SessionNotFound(): super._();







@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HaloApiError_SessionNotFound);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'HaloApiError.sessionNotFound()';
}


}




/// @nodoc


class HaloApiError_Core extends HaloApiError {
  const HaloApiError_Core({required this.message}): super._();


 final  String message;

/// Create a copy of HaloApiError
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HaloApiError_CoreCopyWith<HaloApiError_Core> get copyWith => _$HaloApiError_CoreCopyWithImpl<HaloApiError_Core>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HaloApiError_Core&&(identical(other.message, message) || other.message == message));
}


@override
int get hashCode => Object.hash(runtimeType,message);

@override
String toString() {
  return 'HaloApiError.core(message: $message)';
}


}

/// @nodoc
abstract mixin class $HaloApiError_CoreCopyWith<$Res> implements $HaloApiErrorCopyWith<$Res> {
  factory $HaloApiError_CoreCopyWith(HaloApiError_Core value, $Res Function(HaloApiError_Core) _then) = _$HaloApiError_CoreCopyWithImpl;
@useResult
$Res call({
 String message
});




}
/// @nodoc
class _$HaloApiError_CoreCopyWithImpl<$Res>
    implements $HaloApiError_CoreCopyWith<$Res> {
  _$HaloApiError_CoreCopyWithImpl(this._self, this._then);

  final HaloApiError_Core _self;
  final $Res Function(HaloApiError_Core) _then;

/// Create a copy of HaloApiError
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? message = null,}) {
  return _then(HaloApiError_Core(
message: null == message ? _self.message : message // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class HaloApiError_InternalState extends HaloApiError {
  const HaloApiError_InternalState(): super._();







@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HaloApiError_InternalState);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'HaloApiError.internalState()';
}


}




// dart format on
